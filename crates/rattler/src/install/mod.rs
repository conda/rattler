mod driver;
pub mod link;
mod python;
mod transaction;

pub use driver::InstallDriver;
pub use link::{link_file, LinkFileError};
pub use transaction::{Transaction, TransactionOperation};

use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
pub use python::PythonInfo;
use rattler_conda_types::package::{IndexJson, PackageFile};
use rattler_conda_types::prefix_record::PathsEntry;
use rattler_conda_types::{package::PathsJson, Platform};
use std::cmp::Ordering;
use std::collections::binary_heap::PeekMut;
use std::collections::BinaryHeap;
use std::{
    future::ready,
    path::{Path, PathBuf},
};
use tokio::task::JoinError;
use tracing::instrument;

/// An error that might occur when installing a package.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("the operation was cancelled")]
    Cancelled,

    #[error("failed to read 'paths.json'")]
    FailedToReadPathsJson(#[source] std::io::Error),

    #[error("failed to read 'index.json'")]
    FailedToReadIndexJson(#[source] std::io::Error),

    #[error("failed to link '{0}'")]
    FailedToLink(PathBuf, #[source] LinkFileError),

    #[error("target prefix is not UTF-8")]
    TargetPrefixIsNotUtf8,

    #[error("failed to create target directory")]
    FailedToCreateTargetDirectory(#[source] std::io::Error),

    #[error("cannot install noarch python package because there is no python version specified")]
    MissingPythonInfo,
}

impl From<JoinError> for InstallError {
    fn from(err: JoinError) -> Self {
        if let Ok(panic) = err.try_into_panic() {
            std::panic::resume_unwind(panic)
        } else {
            InstallError::Cancelled
        }
    }
}

/// Additional options to pass to [`link_package`] to modify the installation process. Using
/// [`InstallOptions::default`] works in most cases unless you want specific control over the
/// installation process.
#[derive(Default, Clone)]
pub struct InstallOptions {
    /// When files are copied/linked to the target directory hardcoded paths in these files are
    /// "patched". The hardcoded paths are replaced with the full path of the target directory, also
    /// called the "prefix".
    ///
    /// However, in exceptional cases you might want to use a different prefix than the one that is
    /// being installed to. This field allows you to do that. When its set this is used instead of
    /// the target directory.
    pub target_prefix: Option<PathBuf>,

    /// Instead of reading the `paths.json` file from the package directory itself, use the data
    /// specified here.
    ///
    /// This is sometimes useful to avoid reading the file twice or when you want to modify
    /// installation process externally.
    pub paths_json: Option<PathsJson>,

    /// Instead of reading the `index.json` file from the package directory itself, use the data
    /// specified here.
    ///
    /// This is sometimes useful to avoid reading the file twice or when you want to modify
    /// installation process externally.
    pub index_json: Option<IndexJson>,

    /// Whether or not to use symbolic links where possible. If this is set to `Some(false)`
    /// symlinks are disabled, if set to `Some(true)` symbolic links are alwas used when specified
    /// in the [`info/paths.json`] file even if this is not supported. If the value is set to `None`
    /// symbolic links are only used if they are supported.
    ///
    /// Windows only supports symbolic links in specific cases.
    pub allow_symbolic_links: Option<bool>,

    /// Whether or not to use hard links where possible. If this is set to `Some(false)` the use of
    /// hard links is disabled, if set to `Some(true)` hard links are always used when specified
    /// in the [`info/paths.json`] file even if this is not supported. If the value is set to `None`
    /// hard links are only used if they are supported. A dummy hardlink is created to determine
    /// support.
    ///
    /// Hard links are supported by most OSes but often require that the hard link and its content
    /// are on the same filesystem.
    pub allow_hard_links: Option<bool>,

    /// The platform for which the package is installed. Some operations like signing require
    /// different behavior depending on the platform. If the field is set to `None` the current
    /// platform is used.
    pub platform: Option<Platform>,

    /// Python version information of the python distribution installed within the environment. This
    /// is only used when installing noarch Python packages. Noarch python packages are python
    /// packages that contain python source code that has to be installed in the correct
    /// site-packages directory based on the version of python. This site-packages directory depends
    /// on the version of python, therefor it must be provided when linking.
    ///
    /// If you're installing a noarch python package and do not provide this field, the
    /// [`link_package`] function will return [`InstallError::MissingPythonInfo`].
    pub python_info: Option<PythonInfo>,
}

/// Given an extracted package archive (`package_dir`), installs its files to the `target_dir`.
///
/// Returns a [`PathsEntry`] for every file that was linked into the target directory. The entries
/// are ordered in the same order as they appear in the `paths.json` file of the package.
#[instrument(skip_all, fields(package_dir = %package_dir.display()))]
pub async fn link_package(
    package_dir: &Path,
    target_dir: &Path,
    driver: &InstallDriver,
    options: InstallOptions,
) -> Result<Vec<PathsEntry>, InstallError> {
    // Determine the target prefix for linking
    let target_prefix = options
        .target_prefix
        .as_deref()
        .unwrap_or(target_dir)
        .to_str()
        .ok_or(InstallError::TargetPrefixIsNotUtf8)?
        .to_owned();

    // Ensure target directory exists
    tokio::fs::create_dir_all(&target_dir)
        .await
        .map_err(InstallError::FailedToCreateTargetDirectory)?;

    // Reuse or read the `paths.json` and `index.json` files from the package directory
    let paths_json = read_paths_json(package_dir, driver, options.paths_json);
    let index_json = read_index_json(package_dir, driver, options.index_json);
    let (paths_json, index_json) = tokio::try_join!(paths_json, index_json)?;

    // Error out if this is a noarch python package but the python information is missing.
    if index_json.noarch.is_python() && options.python_info.is_none() {
        return Err(InstallError::MissingPythonInfo);
    }

    // Determine whether or not we can use symbolic links
    let (allow_symbolic_links, allow_hard_links) = tokio::join!(
        // Determine if we can use symlinks
        match options.allow_symbolic_links {
            Some(value) => ready(value).left_future(),
            None => can_create_symlinks(target_dir).right_future(),
        },
        // Determine if we can use hard links
        match options.allow_hard_links {
            Some(value) => ready(value).left_future(),
            None => can_create_hardlinks(&paths_json, target_dir, package_dir).right_future(),
        }
    );

    // Determine the platform to use
    let platform = options.platform.unwrap_or(Platform::current());

    // Link all package files in parallel
    let mut link_futures = FuturesUnordered::new();
    for (idx, entry) in paths_json.paths.into_iter().enumerate() {
        let package_dir = package_dir.to_owned();
        let target_dir = target_dir.to_owned();
        let target_prefix = target_prefix.to_owned();
        let python_info = options.python_info.clone();

        // Spawn a task to link the specific file. Note that these tasks are throttled by the
        // driver. So even though we might spawn thousands of tasks they might not all run
        // parallel because the driver dictates that only N tasks can run in parallel at the same
        // time.
        let link_future = driver.spawn_throttled(move || {
            link_file(
                index_json.noarch,
                &entry,
                &package_dir,
                &target_dir,
                &target_prefix,
                allow_symbolic_links && !entry.no_link,
                allow_hard_links && !entry.no_link,
                platform,
                python_info.as_ref(),
            )
            .map_err(|e| InstallError::FailedToLink(entry.relative_path.clone(), e))
            .map(|result| {
                (
                    idx,
                    PathsEntry {
                        relative_path: result.relative_path,
                        path_type: entry.path_type.into(),
                        no_link: entry.no_link,
                        sha256: entry.sha256,
                        sha256_in_prefix: Some(format!("{:x}", result.sha256)),
                        size_in_bytes: Some(result.file_size),
                    },
                )
            })
        });

        // Push back the link future
        link_futures.push(link_future);
    }

    // Await all futures and collect them. The futures are added in order to the `link_futures`
    // set. However, they can complete in any order. This means we have to reorder them back into
    // their original order. This is achieved by waiting to add finished results to the result Vec,
    // if the result before it has not yet finished. To that end we use a `BinaryHeap` as a priority
    // queue which will buffer up finished results that finished before their predecessor.
    //
    // What makes this loop special is that it also aborts if any of the returned futures indicate
    // a failure.
    let mut paths = Vec::with_capacity(link_futures.len());
    let mut out_of_order_queue =
        BinaryHeap::<OrderWrapper<PathsEntry>>::with_capacity(link_futures.len());
    while let Some(link_result) = link_futures.next().await {
        let (index, data) = link_result?;

        if index == paths.len() {
            // If this is the next element expected in the sorted list, add it immediately. This
            // basically means the future finished in order.
            paths.push(data);

            // By adding a finished future we have to check if there might also be another future
            // that finished earlier and should also now be added to the result Vec.
            while let Some(next_output) = out_of_order_queue.peek_mut() {
                if next_output.index == paths.len() {
                    paths.push(PeekMut::pop(next_output).data);
                } else {
                    break;
                }
            }
        } else {
            // Otherwise add it to the out of order queue. This means that we still have to wait for
            // an another element before we can add the result to the ordered list.
            out_of_order_queue.push(OrderWrapper { index, data });
        }
    }
    debug_assert_eq!(
        paths.len(),
        paths.capacity(),
        "some futures where not added to the result"
    );

    Ok(paths)
}

/// A helper function that reads the `paths.json` file from a package unless it has already been
/// provided, in which case it is returned immediately.
async fn read_paths_json(
    package_dir: &Path,
    driver: &InstallDriver,
    paths_json: Option<PathsJson>,
) -> Result<PathsJson, InstallError> {
    match paths_json {
        Some(paths) => Ok(paths),
        None => {
            let package_dir = package_dir.to_owned();
            driver
                .spawn_throttled(move || {
                    PathsJson::from_package_directory_with_deprecated_fallback(&package_dir)
                        .map_err(InstallError::FailedToReadPathsJson)
                })
                .await
        }
    }
}

/// A helper function that reads the `index.json` file from a package unless it has already been
/// provided, in which case it is returned immediately.
async fn read_index_json(
    package_dir: &Path,
    driver: &InstallDriver,
    index_json: Option<IndexJson>,
) -> Result<IndexJson, InstallError> {
    match index_json {
        Some(index) => Ok(index),
        None => {
            let package_dir = package_dir.to_owned();
            driver
                .spawn_throttled(move || {
                    IndexJson::from_package_directory(&package_dir)
                        .map_err(InstallError::FailedToReadPathsJson)
                })
                .await
        }
    }
}

/// A helper struct for a BinaryHeap to provides ordering to items that are otherwise unordered.
struct OrderWrapper<T> {
    index: usize,
    data: T,
}

impl<T> PartialEq for OrderWrapper<T> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

impl<T> Eq for OrderWrapper<T> {}

impl<T> PartialOrd for OrderWrapper<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for OrderWrapper<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max heap, so compare backwards here.
        other.index.cmp(&self.index)
    }
}

/// Returns true if it is possible to create symlinks in the target directory.
async fn can_create_symlinks(target_dir: &Path) -> bool {
    let uuid = uuid::Uuid::new_v4();
    let symlink_path = target_dir.join(format!("symtest_{}", uuid));
    #[cfg(windows)]
    let result = tokio::fs::symlink_file("./", &symlink_path).await;
    #[cfg(unix)]
    let result = tokio::fs::symlink("./", &symlink_path).await;
    match result {
        Ok(_) => {
            if let Err(e) = tokio::fs::remove_file(&symlink_path).await {
                tracing::warn!(
                    "failed to delete temporary file '{}': {e}",
                    symlink_path.display()
                )
            }
            true
        }
        Err(e) => {
            tracing::debug!(
                "failed to create symlink in target directory: {e}. Disabling use of symlinks."
            );
            false
        }
    }
}

/// Returns true if it is possible to create hard links from the target directory to the package
/// cache directory.
async fn can_create_hardlinks(
    paths_json: &PathsJson,
    target_dir: &Path,
    package_dir: &Path,
) -> bool {
    let dst_link_path = target_dir.join(format!("sentinel_{}", uuid::Uuid::new_v4()));
    let src_link_path = match paths_json.paths.first() {
        Some(path) => package_dir.join(&path.relative_path),
        None => return false,
    };
    tokio::task::spawn_blocking(
        move || match std::fs::hard_link(&src_link_path, &dst_link_path) {
            Ok(_) => {
                if let Err(e) = std::fs::remove_file(&dst_link_path) {
                    tracing::warn!(
                        "failed to delete temporary file '{}': {e}",
                        dst_link_path.display()
                    )
                }
                true
            }
            Err(e) => {
                tracing::debug!(
                "failed to create hard link in target directory: {e}. Disabling use of hard links."
            );
                false
            }
        },
    )
    .await
    .unwrap_or(false)
}

#[cfg(test)]
mod test {
    use crate::install::InstallDriver;
    use crate::{
        get_test_data_dir,
        install::{link_package, InstallOptions},
        package_cache::PackageCache,
    };
    use futures::{stream, StreamExt};
    use rattler_conda_types::package::ArchiveIdentifier;
    use rattler_conda_types::{ExplicitEnvironmentSpec, Platform};
    use reqwest::Client;
    use std::env::temp_dir;
    use std::process::Command;
    use tempfile::tempdir;

    #[tracing_test::traced_test]
    #[tokio::test]
    pub async fn test_install_python() {
        // Load a prepared explicit environment file for the current platform.
        let current_platform = Platform::current();
        let explicit_env_path =
            get_test_data_dir().join(format!("python/explicit-env-{current_platform}.txt"));
        let env = ExplicitEnvironmentSpec::from_path(&explicit_env_path).unwrap();

        assert_eq!(env.platform, Some(current_platform), "the platform for which the explicit lock file was created does not match the current platform");

        // Open a package cache in the systems temporary directory with a specific name. This allows
        // us to reuse a package cache across multiple invocations of this test. Useful if you're
        // debugging.
        let package_cache = PackageCache::new(temp_dir().join("rattler/test_install_python_pkgs"));

        // Create an HTTP client we can use to download packages
        let client = Client::new();

        // Download and install each layer into an environment.
        let install_driver = InstallDriver::default();
        let target_dir = tempdir().unwrap();
        stream::iter(env.packages)
            .for_each_concurrent(Some(50), |package_url| {
                let prefix_path = target_dir.path();
                let client = client.clone();
                let package_cache = &package_cache;
                let install_driver = &install_driver;
                async move {
                    // Populate the cache
                    let package_info = ArchiveIdentifier::try_from_url(&package_url.url).unwrap();
                    let package_dir = package_cache
                        .get_or_fetch_from_url(package_info, package_url.url, client.clone())
                        .await
                        .unwrap();

                    // Install the package to the prefix
                    link_package(
                        &package_dir,
                        prefix_path,
                        install_driver,
                        InstallOptions::default(),
                    )
                    .await
                    .unwrap();
                }
            })
            .await;

        // Run the python command and validate the version it outputs
        let python_path = if current_platform.is_windows() {
            "python.exe"
        } else {
            "bin/python"
        };
        let python_version = Command::new(target_dir.path().join(python_path))
            .arg("--version")
            .output()
            .unwrap();

        assert!(python_version.status.success());
        assert_eq!(
            String::from_utf8_lossy(&python_version.stdout).trim(),
            "Python 3.11.0"
        );
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    async fn test_prefix_paths() {
        let environment_dir = tempfile::TempDir::new().unwrap();
        let package_dir = tempfile::TempDir::new().unwrap();

        // Create package cache
        rattler_package_streaming::fs::extract(
            &get_test_data_dir().join("ruff-0.0.171-py310h298983d_0.conda"),
            package_dir.path(),
        )
        .unwrap();

        // Link the package
        let paths = link_package(
            package_dir.path(),
            environment_dir.path(),
            &InstallDriver::default(),
            Default::default(),
        )
        .await
        .unwrap();

        insta::assert_yaml_snapshot!(paths);
    }
}
