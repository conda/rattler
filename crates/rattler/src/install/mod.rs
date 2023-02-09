mod link;

pub use crate::install::link::LinkFileError;
use futures::FutureExt;
use rattler_conda_types::package::PathsJson;
use std::future::ready;
use std::path::{Path, PathBuf};
use tokio::task::JoinError;
use tracing::instrument;

/// An error that might occur when installing a package.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("the operation was cancelled")]
    Cancelled,

    #[error("failed to read 'paths.json'")]
    FailedToReadPathsJson(#[source] std::io::Error),

    #[error("failed to link '{0}'")]
    FailedToLink(PathBuf, #[source] LinkFileError),

    #[error("target prefix is not UTF-8")]
    TargetPrefixIsNotUtf8,

    #[error("failed to create target directory")]
    FailedToCreateTargetDirectory(#[source] std::io::Error),
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

/// Additional options to pass to [`install_package`] to modify the installation process. Using
/// [`InstallOptions::default`] works in most cases unless you want specific control over the
/// installation process.
#[derive(Default)]
pub struct InstallOptions {
    /// When files are copied/linked to the target directory hardcoded paths in these files are
    /// "patched". The hardcoded paths are replaced with the full path of the target directory, also
    /// called the "prefix".
    ///
    /// However, in exceptional cases you might want to use a different prefix than the one that is
    /// being installed to. This field allows you to do that. When its set this is used instead of
    /// the target directory.
    target_prefix: Option<PathBuf>,

    /// Instead of reading the `paths.json` file from the package directory itself, use the data
    /// specified here.
    ///
    /// This is sometimes useful to avoid reading the file twice or when you want to modify
    /// installation process externally.
    paths_json: Option<PathsJson>,

    /// Whether or not to use symbolic links where possible. If this is set to `Some(false)`
    /// symlinks are disabled, if set to `Some(true)` symbolic links are alwas used when specified
    /// in the [`info/paths.json`] file even if this is not supported. If the value is set to `None`
    /// symbolic links are only used if they are supported.
    ///
    /// Windows only supports symbolic links in specific cases.
    allow_symbolic_links: Option<bool>,

    /// Whether or not to use hard links where possible. If this is set to `Some(false)` the use of
    /// hard links is disabled, if set to `Some(true)` hard links are always used when specified
    /// in the [`info/paths.json`] file even if this is not supported. If the value is set to `None`
    /// hard links are only used if they are supported. A dummy hardlink is created to determine
    /// support.
    ///
    /// Hard links are supported by most OSes but often require that the hard link and its content
    /// are on the same filesystem.
    allow_hard_links: Option<bool>,
}

/// Given an extracted package archive (`package_dir`), install its files to the `target_dir`.
#[instrument(skip_all, fields(package_dir = %package_dir.display()))]
pub async fn link_package(
    package_dir: &Path,
    target_dir: &Path,
    options: InstallOptions,
) -> Result<(), InstallError> {
    // Determine the target prefix for linking
    let target_prefix = options
        .target_prefix
        .as_deref()
        .unwrap_or(&target_dir)
        .to_str()
        .ok_or_else(|| InstallError::TargetPrefixIsNotUtf8)?
        .to_owned();

    // Ensure target directory exists
    tokio::fs::create_dir_all(&target_dir)
        .await
        .map_err(InstallError::FailedToCreateTargetDirectory)?;

    // Reuse or read the `paths.json` file from the package directory
    let paths_json = match options.paths_json {
        Some(paths) => paths,
        None => read_paths_from_package_dir(&package_dir).await?,
    };

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

    // Link all package files. Filesystem operations are blocking so we run all of them on another
    // tokio thread.
    let package_dir = package_dir.to_owned();
    let target_dir = target_dir.to_owned();
    tokio::task::spawn_blocking(move || {
        link_package_files(
            &package_dir,
            &target_dir,
            paths_json,
            allow_symbolic_links,
            allow_hard_links,
            &target_prefix,
        )
    })
    .await??;

    Ok(())
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

/// A blocking function to link all files in a package to a target directory.
fn link_package_files(
    package_dir: &PathBuf,
    target_dir: &PathBuf,
    paths_json: PathsJson,
    allow_symbolic_links: bool,
    allow_hard_links: bool,
    target_prefix: &str,
) -> Result<(), InstallError> {
    for entry in paths_json.paths {
        link::link_file(
            &entry.relative_path,
            &package_dir,
            &target_dir,
            &target_prefix,
            entry.prefix_placeholder.as_deref(),
            entry.path_type,
            entry.file_mode,
            allow_symbolic_links,
            allow_hard_links,
        )
        .map_err(|e| InstallError::FailedToLink(entry.relative_path.clone(), e))?;
    }

    Ok(())
}

/// Async version of reading the [`PathsJson`] information from a package directory by spawning
/// a new blocking task. This makes sense because reading the info from disk takes time.
async fn read_paths_from_package_dir(package_dir: &Path) -> Result<PathsJson, InstallError> {
    let package_dir = package_dir.to_owned();
    Ok(tokio::task::spawn_blocking(move || {
        PathsJson::from_package_directory_with_deprecated_fallback(&package_dir)
    })
    .await?
    .map_err(InstallError::FailedToReadPathsJson)?)
}

#[cfg(test)]
mod test {
    use crate::{
        get_test_data_dir,
        install::{link_package, InstallOptions},
        package_cache::{PackageCache, PackageInfo},
    };
    use futures::{stream, StreamExt};
    use rattler_conda_types::{ExplicitEnvironmentSpec, Platform};
    use reqwest::Client;
    use std::{env::temp_dir, process::Command};
    use tempfile::tempdir;

    #[tracing_test::traced_test]
    #[tokio::test(flavor = "current_thread")]
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
        let target_dir = tempdir().unwrap();
        stream::iter(env.packages)
            .for_each_concurrent(Some(50), |package_url| {
                let prefix_path = target_dir.path();
                let client = client.clone();
                let package_cache = &package_cache;
                async move {
                    // Populate the cache
                    let package_info = PackageInfo::try_from_url(&package_url.url).unwrap();
                    let package_dir = package_cache
                        .get_or_fetch_from_url(package_info, package_url.url, client.clone())
                        .await
                        .unwrap();

                    // Install the package to the prefix
                    link_package(&package_dir, prefix_path, InstallOptions::default())
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
}
