use std::{fs::File, path::Path, str::FromStr};

use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none};

/// A representation of the `run_exports.json` file found in package archives.
///
/// The `run_exports.json` file contains information about the run exports of a package
#[serde_as]
#[skip_serializing_none]
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Hash)]
pub struct RunExports {
    // weak run exports apply a dependency from host to run
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub weak: Vec<String>,
    // strong run exports apply a dependency from build to host and run
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub strong: Vec<String>,
    // noarch run exports apply a run export only to noarch packages (other run exports are ignored)
    // for example, python uses this to apply a dependency on python to all noarch packages, but not to
    // the python_abi package
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub noarch: Vec<String>,
    // weak constrains apply a constrain dependency from host to build, or run to host
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub weak_constrains: Vec<String>,
    // strong constrains apply a constrain dependency from build to host and run
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub strong_constrains: Vec<String>,
}

impl RunExports {
    /// Parses a `run_exports.json` file from a reader.
    pub fn from_reader(reader: impl std::io::Read) -> Result<Self, std::io::Error> {
        serde_json::from_reader(reader).map_err(Into::into)
    }

    /// Parses a `run_exports.json` file from a file.
    pub fn from_path(path: &Path) -> Result<Self, std::io::Error> {
        Self::from_reader(File::open(path)?)
    }

    /// Reads the file from a package archive directory
    pub fn from_package_directory(path: &Path) -> Result<Self, std::io::Error> {
        Self::from_path(&path.join("info/run_exports.json"))
    }
}

impl FromStr for RunExports {
    type Err = std::io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s).map_err(Into::into)
    }
}

#[cfg(all(unix, test))]
mod test {
    use super::RunExports;

    #[test]
    pub fn test_reconstruct_run_exports_json_with_symlinks() {
        let package_dir = tempfile::tempdir().unwrap();
        rattler_package_streaming::fs::extract(
            &crate::get_test_data_dir().join("libzlib-1.2.13-hfd90126_4.tar.bz2"),
            package_dir.path(),
        )
        .unwrap();

        let package_dir = package_dir.into_path();
        println!("{}", package_dir.display());

        insta::assert_yaml_snapshot!(RunExports::from_package_directory(&package_dir).unwrap());
    }
}
