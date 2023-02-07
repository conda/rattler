use rattler_conda_types::package::PathsJson;
use std::path::{Path, PathBuf};

/// An error that might occur when installing a package.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("failed to read 'paths.json'")]
    FailedToReadPathsJson(#[source] std::io::Error),
}

/// Additional options to pass to [`install_package`] to modify the installation process.
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
}

/// Given an extracted package archive (`package_dir`), install its files to the `target_dir`.
pub async fn install_package(
    package_dir: &Path,
    target_dir: &Path,
    options: InstallOptions,
) -> Result<(), InstallError> {
    // Use the passed in paths.json or read it from the package directory.
    let paths_json = match options.paths_json {
        Some(paths) => paths,
        None => read_paths_from_package_dir(package_dir)
            .await
            .map_err(InstallError::FailedToReadPathsJson)?,
    };

    // Iterate over all files advertised in the paths.json file.

    Ok(())
}

async fn read_paths_from_package_dir(package_dir: &Path) -> Result<PathsJson, std::io::Error> {
    let package_dir = package_dir.to_owned();
    Ok(tokio::task::spawn_blocking(move || {
        PathsJson::from_package_directory_with_deprecated_fallback(&package_dir)
    })
    .await??)
}

#[cfg(test)]
mod test {
    use crate::get_test_data_dir;
    use crate::install::{install_package, InstallOptions};
    use crate::package_cache::{PackageCache, PackageInfo};
    use rattler_conda_types::{ExplicitEnvironmentSpec, Platform};
    use reqwest::Client;
    use std::env::temp_dir;
    use tempfile::tempdir;

    #[tracing_test::traced_test]
    #[tokio::test]
    pub async fn test_install_python() {
        // Load a prepared explicit environment file for the current platform.
        let current_platform = Platform::current();
        let explicit_env_path =
            get_test_data_dir().join(format!("python/explicit-env-{current_platform}.txt"));
        let env = ExplicitEnvironmentSpec::from_path(&explicit_env_path).unwrap();
        assert_eq!(env.platform, Some(current_platform));

        let package_cache_path =
            PackageCache::new(temp_dir().join("rattler/test_install_python_pkgs"));
        let prefix_path = tempdir().unwrap();
        let client = Client::new();

        // Download and install each layer into an environment
        for package_url in env.packages {
            // Populate the cache
            let package_info = PackageInfo::try_from_url(&package_url.url).unwrap();
            let package_dir = package_cache_path
                .get_or_fetch_from_url(package_info, package_url.url, client.clone())
                .await
                .unwrap();

            // Install the package to the prefix
            install_package(&package_dir, prefix_path.path(), InstallOptions::default())
                .await
                .unwrap();
        }

        // TODO: Run the python command and validate the version
        // asdas
    }
}
