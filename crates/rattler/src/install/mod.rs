//! This module contains the logic to install a package into a prefix. The main entry point is the
//! [`link_package`] function.
//!
//! The [`link_package`] function takes a package directory and a target directory. The package
//! directory is the directory that contains the extracted package archive. The target directory is
//! the directory into which the package should be installed. The target directory is also called
//! the "prefix".
//!
//! The [`link_package`] function will read the `paths.json` file from the package directory and
//! link all files specified in that file into the target directory. The `paths.json` file contains
//! a list of files that should be installed and how they should be installed. For example, the
//! `paths.json` file might contain a file that should be copied into the target directory. Or it
//! might contain a file that should be linked into the target directory. The `paths.json` file
//! also contains a SHA256 hash for each file. This hash is used to verify that the file was not
//! tampered with.
pub mod apple_codesign;
mod driver;
mod entry_point;
pub mod link;
mod python;
mod transaction;

pub use crate::install::entry_point::python_entry_point_template;
pub use driver::InstallDriver;
pub use link::{link_file, LinkFileError};
pub use transaction::{Transaction, TransactionError, TransactionOperation};

use crate::install::entry_point::{
    create_unix_python_entry_point, create_windows_python_entry_point,
};
pub use apple_codesign::AppleCodeSignBehavior;
use futures::FutureExt;
pub use python::PythonInfo;
use rattler_conda_types::package::{IndexJson, LinkJson, NoArchLinks, PackageFile};
use rattler_conda_types::prefix_record::PathsEntry;
use rattler_conda_types::{package::PathsJson, Platform};
use std::cmp::Ordering;
use std::collections::binary_heap::PeekMut;
use std::collections::BinaryHeap;
use std::io::ErrorKind;
use std::sync::Arc;
use std::{
    future::ready,
    path::{Path, PathBuf},
};
use tokio::task::JoinError;
use tracing::instrument;

/// An error that might occur when installing a package.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// The operation was cancelled.
    #[error("the operation was cancelled")]
    Cancelled,

    /// The paths.json file could not be read.
    #[error("failed to read 'paths.json'")]
    FailedToReadPathsJson(#[source] std::io::Error),

    /// The index.json file could not be read.
    #[error("failed to read 'index.json'")]
    FailedToReadIndexJson(#[source] std::io::Error),

    /// The link.json file could not be read.
    #[error("failed to read 'link.json'")]
    FailedToReadLinkJson(#[source] std::io::Error),

    /// A file could not be linked.
    #[error("failed to link '{0}'")]
    FailedToLink(PathBuf, #[source] LinkFileError),

    /// The target prefix is not UTF-8.
    #[error("target prefix is not UTF-8")]
    TargetPrefixIsNotUtf8,

    /// Failed to create the target directory.
    #[error("failed to create target directory")]
    FailedToCreateTargetDirectory(#[source] std::io::Error),

    /// A noarch package could not be installed because no python version was specified.
    #[error("cannot install noarch python package because there is no python version specified")]
    MissingPythonInfo,

    /// Failed to create a python entry point for a noarch package.
    #[error("failed to create Python entry point")]
    FailedToCreatePythonEntryPoint(#[source] std::io::Error),
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

    /// Instead of reading the `link.json` file from the package directory itself, use the data
    /// specified here.
    ///
    /// This is sometimes useful to avoid reading the file twice or when you want to modify
    /// installation process externally.
    ///
    /// Because the the `link.json` file is optional this fields is using a doubly wrapped Option.
    /// The first `Option` is to indicate whether or not this value is set. The second Option is the
    /// [`LinkJson`] to use or `None` if you want to force that there is no [`LinkJson`].
    ///
    /// This struct is only used if the package to be linked is a noarch Python package.
    pub link_json: Option<Option<LinkJson>>,

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

    /// For binaries on macOS ARM64 (Apple Silicon), binaries need to be signed with an ad-hoc
    /// certificate to properly work. This field controls wether or not to do that.
    /// Code signing is only executed when the target platform is macOS ARM64. By default,
    /// codesigning will fail the installation if it fails. This behavior can be changed by setting
    /// this field to `AppleCodeSignBehavior::Ignore` or `AppleCodeSignBehavior::DoNothing`.
    ///
    /// To sign the binaries, the `/usr/bin/codesign` executable is called with `--force` and
    /// `--sign -` arguments. The `--force` argument is used to overwrite existing signatures, and
    /// the `--sign -` argument is used to sign with an ad-hoc certificate.
    /// Ad-hoc signing does not use an identity at all, and identifies exactly one instance of code.
    pub apple_codesign_behavior: AppleCodeSignBehavior,
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

    // Parse the `link.json` file and extract entry points from it.
    let link_json = if index_json.noarch.is_python() {
        read_link_json(package_dir, driver, options.link_json).await?
    } else {
        None
    };

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

    // Construct a channel to will hold the results of the different linking stages
    let (tx, mut rx) = tokio::sync::mpsc::channel(driver.concurrency_limit());

    // Wrap the python info in an `Arc` so we can more easily share it with async tasks.
    let python_info = options.python_info.map(Arc::new);

    // Start linking all package files in parallel
    let mut number_of_paths_entries = 0;
    for entry in paths_json.paths.into_iter() {
        let package_dir = package_dir.to_owned();
        let target_dir = target_dir.to_owned();
        let target_prefix = target_prefix.to_owned();
        let python_info = python_info.clone();

        // Spawn a task to link the specific file. Note that these tasks are throttled by the
        // driver. So even though we might spawn thousands of tasks they might not all run
        // parallel because the driver dictates that only N tasks can run in parallel at the same
        // time.
        let tx = tx.clone();
        driver.spawn_throttled_and_forget(move || {
            // Return immediately if the receiver was closed. This can happen if a previous step
            // failed. In that case we do not want to continue the installation.
            if tx.is_closed() {
                return;
            }

            let linked_file_result = match link_file(
                index_json.noarch,
                &entry,
                &package_dir,
                &target_dir,
                &target_prefix,
                allow_symbolic_links && !entry.no_link,
                allow_hard_links && !entry.no_link,
                platform,
                python_info.as_deref(),
                options.apple_codesign_behavior,
            ) {
                Ok(result) => Ok((
                    number_of_paths_entries,
                    PathsEntry {
                        relative_path: result.relative_path,
                        path_type: entry.path_type.into(),
                        no_link: entry.no_link,
                        sha256: entry.sha256,
                        sha256_in_prefix: Some(result.sha256),
                        size_in_bytes: Some(result.file_size),
                    },
                )),
                Err(e) => Err(InstallError::FailedToLink(entry.relative_path.clone(), e)),
            };

            // Send the result to the main task for further processing.
            let _ = tx.blocking_send(linked_file_result);
        });

        number_of_paths_entries += 1;
    }

    // If this package is a noarch python package we also have to create entry points.
    //
    // Be careful with the fact that this code is currently running in parallel with the linking of
    // individual files.
    if let Some(link_json) = link_json {
        // Parse the `link.json` file and extract entry points from it.
        let entry_points = match link_json.noarch {
            NoArchLinks::Python(entry_points) => entry_points.entry_points,
            NoArchLinks::Generic => {
                unreachable!("we only use link.json for noarch: python packages")
            }
        };

        // Get python info
        let python_info = python_info
            .clone()
            .expect("should be safe because its checked above that this contains a value");

        // Create entry points for each listed item. This is different between Windows and unix
        // because on Windows, two PathEntry's are created whereas on Linux only one is created.
        for entry_point in entry_points {
            let tx = tx.clone();
            let python_info = python_info.clone();
            let target_dir = target_dir.to_owned();
            let target_prefix = target_prefix.to_owned();

            if platform.is_windows() {
                driver.spawn_throttled_and_forget(move || {
                    // Return immediately if the receiver was closed. This can happen if a previous step
                    // failed. In that case we do not want to continue the installation.
                    if tx.is_closed() {
                        return;
                    }

                    match create_windows_python_entry_point(
                        &target_dir,
                        &target_prefix,
                        &entry_point,
                        &python_info,
                    ) {
                        Ok([a, b]) => {
                            let _ = tx.blocking_send(Ok((number_of_paths_entries, a)));
                            let _ = tx.blocking_send(Ok((number_of_paths_entries + 1, b)));
                        }
                        Err(e) => {
                            let _ = tx.blocking_send(Err(
                                InstallError::FailedToCreatePythonEntryPoint(e),
                            ));
                        }
                    }
                });
                number_of_paths_entries += 2
            } else {
                driver.spawn_throttled_and_forget(move || {
                    // Return immediately if the receiver was closed. This can happen if a previous step
                    // failed. In that case we do not want to continue the installation.
                    if tx.is_closed() {
                        return;
                    }

                    let result = match create_unix_python_entry_point(
                        &target_dir,
                        &target_prefix,
                        &entry_point,
                        &python_info,
                    ) {
                        Ok(a) => Ok((number_of_paths_entries, a)),
                        Err(e) => Err(InstallError::FailedToCreatePythonEntryPoint(e)),
                    };

                    let _ = tx.blocking_send(result);
                });
                number_of_paths_entries += 1;
            }
        }
    }

    // Drop the transmitter on the current task. This ensures that the only alive transmitters are
    // owned by tasks that are running in the background. When we try to receive stuff over the
    // channel we can then know that all tasks are done if all senders are dropped.
    drop(tx);

    // Await the result of all the background tasks. The background tasks are scheduled in order,
    // however, they can complete in any order. This means we have to reorder them back into
    // their original order. This is achieved by waiting to add finished results to the result Vec,
    // if the result before it has not yet finished. To that end we use a `BinaryHeap` as a priority
    // queue which will buffer up finished results that finished before their predecessor.
    //
    // What makes this loop special is that it also aborts if any of the returned results indicate
    // a failure.
    let mut paths = Vec::with_capacity(number_of_paths_entries);
    let mut out_of_order_queue =
        BinaryHeap::<OrderWrapper<PathsEntry>>::with_capacity(driver.concurrency_limit());
    while let Some(link_result) = rx.recv().await {
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
                    IndexJson::from_package_directory(package_dir)
                        .map_err(InstallError::FailedToReadIndexJson)
                })
                .await
        }
    }
}

/// A helper function that reads the `link.json` file from a package unless it has already been
/// provided, in which case it is returned immediately.
async fn read_link_json(
    package_dir: &Path,
    driver: &InstallDriver,
    index_json: Option<Option<LinkJson>>,
) -> Result<Option<LinkJson>, InstallError> {
    match index_json {
        Some(index) => Ok(index),
        None => {
            let package_dir = package_dir.to_owned();
            driver
                .spawn_throttled(move || {
                    LinkJson::from_package_directory(package_dir)
                        .map_or_else(
                            |e| {
                                // Its ok if the file is not present.
                                if e.kind() == ErrorKind::NotFound {
                                    Ok(None)
                                } else {
                                    Err(e)
                                }
                            },
                            |link_json| Ok(Some(link_json)),
                        )
                        .map_err(InstallError::FailedToReadLinkJson)
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
    use crate::install::{InstallDriver, PythonInfo};
    use crate::{
        get_test_data_dir,
        install::{link_package, InstallOptions},
        package_cache::PackageCache,
    };
    use futures::{stream, StreamExt};
    use itertools::Itertools;
    use rattler_conda_types::package::ArchiveIdentifier;
    use rattler_conda_types::{ExplicitEnvironmentSpec, Platform, Version};
    use rattler_lock::CondaLock;
    use rattler_networking::AuthenticatedClient;

    use std::env::temp_dir;
    use std::process::Command;
    use std::str::FromStr;
    use tempfile::tempdir;
    use url::Url;

    #[tracing_test::traced_test]
    #[tokio::test]
    pub async fn test_explicit_lock() {
        // Load a prepared explicit environment file for the current platform.
        let current_platform = Platform::current();
        let explicit_env_path =
            get_test_data_dir().join(format!("python/explicit-env-{current_platform}.txt"));
        let env = ExplicitEnvironmentSpec::from_path(&explicit_env_path).unwrap();

        assert_eq!(env.platform, Some(current_platform), "the platform for which the explicit lock file was created does not match the current platform");

        test_install_python(
            env.packages.iter().map(|p| &p.url),
            "explicit",
            current_platform,
        )
        .await;
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    pub async fn test_conda_lock() {
        // Load a prepared explicit environment file for the current platform.
        let lock_path = get_test_data_dir().join("conda-lock/python-conda-lock.yml");
        let lock = CondaLock::from_path(&lock_path).unwrap();

        let current_platform = Platform::current();
        assert!(lock.metadata.platforms.iter().contains(&current_platform), "the platform for which the explicit lock file was created does not match the current platform");

        test_install_python(
            lock.get_packages_by_platform(current_platform)
                .filter_map(|p| Some(&p.as_conda()?.url)),
            "conda-lock",
            current_platform,
        )
        .await;
    }

    pub async fn test_install_python(
        urls: impl Iterator<Item = &Url>,
        cache_name: &str,
        platform: Platform,
    ) {
        // Open a package cache in the systems temporary directory with a specific name. This allows
        // us to reuse a package cache across multiple invocations of this test. Useful if you're
        // debugging.
        let package_cache = PackageCache::new(temp_dir().join("rattler").join(cache_name));

        // Create an HTTP client we can use to download packages
        let client = AuthenticatedClient::default();

        // Specify python version
        let python_version =
            PythonInfo::from_version(&Version::from_str("3.11.0").unwrap(), platform).unwrap();

        // Download and install each layer into an environment.
        let install_driver = InstallDriver::default();
        let target_dir = tempdir().unwrap();
        stream::iter(urls)
            .for_each_concurrent(Some(50), |package_url| {
                let prefix_path = target_dir.path();
                let client = client.clone();
                let package_cache = &package_cache;
                let install_driver = &install_driver;
                let python_version = &python_version;
                async move {
                    // Populate the cache
                    let package_info = ArchiveIdentifier::try_from_url(package_url).unwrap();
                    let package_dir = package_cache
                        .get_or_fetch_from_url(package_info, package_url.clone(), client.clone())
                        .await
                        .unwrap();

                    // Install the package to the prefix
                    link_package(
                        &package_dir,
                        prefix_path,
                        install_driver,
                        InstallOptions {
                            python_info: Some(python_version.clone()),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
                }
            })
            .await;

        // Run the python command and validate the version it outputs
        let python_path = if Platform::current().is_windows() {
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
