use std::path::Path;

use super::PackageFile;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none};

/// A representation of the `run_exports.json` file found in package archives.
///
/// The `run_exports.json` file contains information about the run exports of a package
#[serde_as]
#[skip_serializing_none]
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Hash, Clone)]
pub struct RunExportsJson {
    /// weak run exports apply a dependency from host to run
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub weak: Vec<String>,
    /// strong run exports apply a dependency from build to host and run
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub strong: Vec<String>,
    /// noarch run exports apply a run export only to noarch packages (other run exports are ignored)
    /// for example, python uses this to apply a dependency on python to all noarch packages, but not to
    /// the python_abi package
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub noarch: Vec<String>,
    /// weak constrains apply a constrain dependency from host to build, or run to host
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub weak_constrains: Vec<String>,
    /// strong constrains apply a constrain dependency from build to host and run
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub strong_constrains: Vec<String>,
}

impl PackageFile for RunExportsJson {
    fn package_path() -> &'static Path {
        Path::new("info/run_exports.json")
    }

    fn from_str(str: &str) -> Result<Self, std::io::Error> {
        serde_json::from_str(str).map_err(Into::into)
    }
}

#[cfg(all(unix, test))]
mod test {
    use super::{PackageFile, RunExportsJson};

    #[test]
    pub fn test_reconstruct_run_exports_json_with_symlinks() {
        let package_dir = tempfile::tempdir().unwrap();
        rattler_package_streaming::fs::extract(
            &crate::get_test_data_dir().join("with-symlinks/libzlib-1.2.13-hfd90126_4.tar.bz2"),
            package_dir.path(),
        )
        .unwrap();

        let package_dir = package_dir.into_path();
        println!("{}", package_dir.display());

        insta::assert_yaml_snapshot!(RunExportsJson::from_package_directory(&package_dir).unwrap());
    }
}
