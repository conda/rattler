use std::{collections::HashMap, fs::File, path::Path, str::FromStr};

use crate::package::RunExports;

use crate::Version;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none, DisplayFromStr};

/// A representation of the `index.json` file found in package archives.
///
/// The `index.json` file contains information about the package build and dependencies of the package.
/// This data makes up the repodata.json file in the repository.
#[serde_as]
#[skip_serializing_none]
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct Index {
    /// The lowercase name of the package
    pub name: String,

    /// The version of the package
    #[serde_as(as = "DisplayFromStr")]
    pub version: Version,

    /// The build string of the package.
    pub build: String,

    /// The build number of the package. This is also included in the build string.
    pub build_number: usize,

    /// Optionally, the architecture the package is build for.
    pub arch: Option<String>,

    /// Optionally, the OS the package is build for.
    pub platform: Option<String>,

    /// Optionally, the license
    pub license: Option<String>,

    /// Optionally, the license family
    pub license_family: Option<String>,

    /// The dependencies of the package
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends: Vec<String>,

    /// The package constraints of the package
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constrains: Vec<String>,

    /// Any run_exports contained within the package.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    #[serde_as(as = "HashMap<DisplayFromStr, _>")]
    pub run_exports: HashMap<Version, RunExports>,

    /// The timestamp when this package was created
    pub timestamp: Option<u64>,

    /// The subdirectory that contains this package
    pub subdir: Option<String>,
}

impl Index {
    /// Parses a `index.json` file from a reader.
    pub fn from_reader(reader: impl std::io::Read) -> Result<Self, std::io::Error> {
        serde_json::from_reader(reader).map_err(Into::into)
    }

    /// Parses a `index.json` file from a file.
    pub fn from_path(path: &Path) -> Result<Self, std::io::Error> {
        Self::from_reader(File::open(path)?)
    }

    /// Reads the file from a package archive directory
    pub fn from_package_directory(path: &Path) -> Result<Self, std::io::Error> {
        Self::from_path(&path.join("info/index.json"))
    }
}

impl FromStr for Index {
    type Err = std::io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s).map_err(Into::into)
    }
}

#[cfg(test)]
mod test {
    use super::Index;

    #[test]
    pub fn test_reconstruct_index_json() {
        let package_dir = tempfile::tempdir().unwrap();
        rattler_package_streaming::fs::extract(
            &crate::get_test_data_dir().join("zlib-1.2.8-vc10_0.tar.bz2"),
            package_dir.path(),
        )
        .unwrap();

        insta::assert_yaml_snapshot!(Index::from_package_directory(package_dir.path()).unwrap());
    }

    #[test]
    #[cfg(unix)]
    pub fn test_reconstruct_index_json_with_symlinks() {
        let package_dir = tempfile::tempdir().unwrap();
        rattler_package_streaming::fs::extract(
            &crate::get_test_data_dir().join("with-symlinks/zlib-1.2.8-3.tar.bz2"),
            package_dir.path(),
        )
        .unwrap();

        let package_dir = package_dir.into_path();
        println!("{}", package_dir.display());

        insta::assert_yaml_snapshot!(Index::from_package_directory(&package_dir).unwrap());
    }
}
