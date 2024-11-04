use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

use rattler_conda_types::{PackageRecord, Platform, Version};

/// Information required for linking no-arch python packages. The struct
/// contains information about a specific Python version that is installed in an
/// environment.
#[derive(Debug, Clone)]
pub struct PythonInfo {
    /// The platform that the python package is installed for
    pub platform: Platform,

    /// The major and minor version
    pub short_version: (u64, u64),

    /// The relative path to the python executable
    pub path: PathBuf,

    /// The relative path to where site-packages are stored
    pub site_packages_path: PathBuf,

    /// Path to the binary directory
    pub bin_dir: PathBuf,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum PythonInfoError {
    #[error("invalid python version '{0}'")]
    InvalidVersion(String),
}

impl PythonInfo {
    /// Build an instance based on metadata of the package that represents the
    /// python interpreter.
    pub fn from_python_record(
        record: &PackageRecord,
        platform: Platform,
    ) -> Result<Self, PythonInfoError> {
        Self::from_version(
            record.version.version(),
            record.python_site_packages_path.as_deref(),
            platform,
        )
    }

    /// Build an instance based on the version of the python package and the
    /// platform it is installed for.
    pub fn from_version(
        version: &Version,
        site_packages_path: Option<&str>,
        platform: Platform,
    ) -> Result<Self, PythonInfoError> {
        // Determine the major, and minor versions of the version
        let (major, minor) = version
            .as_major_minor()
            .ok_or_else(|| PythonInfoError::InvalidVersion(version.to_string()))?;

        // Determine the expected relative path of the executable in a prefix
        let path = if platform.is_windows() {
            PathBuf::from("python.exe")
        } else {
            PathBuf::from(format!("bin/python{major}.{minor}"))
        };

        // Find the location of the site packages
        let site_packages_path = site_packages_path.map_or_else(
            || {
                if platform.is_windows() {
                    PathBuf::from("Lib/site-packages")
                } else {
                    PathBuf::from(format!("lib/python{major}.{minor}/site-packages"))
                }
            },
            PathBuf::from,
        );

        // Binary directory
        let bin_dir = if platform.is_windows() {
            PathBuf::from("Scripts")
        } else {
            PathBuf::from("bin")
        };

        Ok(Self {
            platform,
            short_version: (major, minor),
            path,
            site_packages_path,
            bin_dir,
        })
    }

    /// Returns the path to the python executable
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Constructs a shebang that will run the rest of the script as Python.
    pub fn shebang(&self, target_prefix: &str) -> String {
        let target_path = Path::new(target_prefix).join(self.path());
        let target_path = target_path.as_os_str().to_string_lossy().replace('\\', "/");

        // Shebangs cannot be larger than 127 characters and executables with spaces are
        // problematic.
        if target_path.len() > 127 - 2 || target_path.contains(' ') {
            format!(
                "#!/bin/sh\n'''exec' \"{}\" \"$0\" \"$@\" #'''",
                &target_path
            )
        } else {
            format!("#!{}", &target_path)
        }
    }

    /// Returns the target location of a file in a noarch python package given
    /// its location in its package archive.
    pub fn get_python_noarch_target_path<'a>(&self, relative_path: &'a Path) -> Cow<'a, Path> {
        if let Ok(rest) = relative_path.strip_prefix("site-packages/") {
            self.site_packages_path.join(rest).into()
        } else if let Ok(rest) = relative_path.strip_prefix("python-scripts/") {
            self.bin_dir.join(rest).into()
        } else {
            relative_path.into()
        }
    }

    /// Returns true if this version of python differs so much that a relink is
    /// required for all noarch python packages.
    pub fn is_relink_required(&self, previous: &PythonInfo) -> bool {
        self.short_version.0 != previous.short_version.0
            || self.short_version.1 != previous.short_version.1
    }
}
