use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none};

use super::PackageFile;

/// A representation of the `run_exports.json` file found in package archives.
///
/// The `run_exports.json` file contains information about the run exports of a
/// package
#[serde_as]
#[skip_serializing_none]
#[derive(Debug, Default, Deserialize, Serialize, Eq, PartialEq, Hash, Clone)]
pub struct RunExportsJson {
    /// weak run exports apply a dependency from host to run
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub weak: Vec<String>,
    /// strong run exports apply a dependency from build to host and run
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub strong: Vec<String>,
    /// noarch run exports apply a run export only to noarch packages (other run
    /// exports are ignored) for example, python uses this to apply a
    /// dependency on python to all noarch packages, but not to
    /// the python_abi package
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub noarch: Vec<String>,
    /// weak constrains apply a constrain dependency from host to build, or run
    /// to host
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub weak_constrains: Vec<String>,
    /// strong constrains apply a constrain dependency from build to host and
    /// run
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

impl RunExportsJson {
    /// Construct an empty `RunExportsJson`
    pub fn new() -> Self {
        Self::default()
    }

    /// Test if all fields are empty
    pub fn is_empty(&self) -> bool {
        self.weak.is_empty()
            && self.strong.is_empty()
            && self.noarch.is_empty()
            && self.weak_constrains.is_empty()
            && self.strong_constrains.is_empty()
    }
}

#[cfg(all(unix, test))]
mod test {
    use super::{PackageFile, RunExportsJson};

    #[test]
    pub fn test_reconstruct_run_exports_json_with_symlinks() {
        let package_dir = tempfile::tempdir().unwrap();

        let package_path = tools::download_and_cache_file(
            "https://conda.anaconda.org/conda-forge/osx-64/zlib-1.2.12-hfd90126_4.tar.bz2"
                .parse()
                .unwrap(),
            "81592fa07b17ecb26813a3238e198b9d1fe39b77628b3f68744bffbaac505e93",
        )
        .unwrap();
        rattler_package_streaming::fs::extract(&package_path, package_dir.path()).unwrap();

        let package_dir = package_dir.into_path();
        println!("{}", package_dir.display());

        insta::assert_yaml_snapshot!(RunExportsJson::from_package_directory(&package_dir).unwrap());
    }
}
