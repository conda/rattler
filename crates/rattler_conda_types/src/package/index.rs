use std::path::Path;

use rattler_macros::sorted;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none, OneOrMany};

use super::PackageFile;
use crate::{NoArchType, PackageName, VersionWithSource};

/// A representation of the `index.json` file found in package archives.
///
/// The `index.json` file contains information about the package build and
/// dependencies of the package. This data makes up the repodata.json file in
/// the repository.
#[serde_as]
#[sorted]
#[skip_serializing_none]
#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub struct IndexJson {
    /// Optionally, the architecture the package is build for.
    pub arch: Option<String>,

    /// The build string of the package.
    pub build: String,

    /// The build number of the package. This is also included in the build
    /// string.
    pub build_number: u64,

    /// The package constraints of the package
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constrains: Vec<String>,

    /// The dependencies of the package
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends: Vec<String>,

    /// Features are a deprecated way to specify different feature sets for the
    /// conda solver. This is not supported anymore and should not be used.
    /// Instead, `mutex` packages should be used to specify
    /// mutually exclusive features.
    pub features: Option<String>,

    /// Optionally, the license
    pub license: Option<String>,

    /// Optionally, the license family
    pub license_family: Option<String>,

    /// The lowercase name of the package
    pub name: PackageName,

    /// If this package is independent of architecture this field specifies in
    /// what way. See [`NoArchType`] for more information.
    #[serde(skip_serializing_if = "NoArchType::is_none")]
    pub noarch: NoArchType,

    /// Optionally, the OS the package is build for.
    pub platform: Option<String>,

    /// Optionally a path within the environment of the site-packages directory.
    /// This field is only present for python interpreter packages.
    /// This field was introduced with <https://github.com/conda/ceps/blob/main/cep-17.md>.
    pub python_site_packages_path: Option<String>,

    /// The subdirectory that contains this package
    pub subdir: Option<String>,

    /// The timestamp when this package was created
    #[serde_as(as = "Option<crate::utils::serde::Timestamp>")]
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,

    /// Track features are nowadays only used to downweight packages (ie. give
    /// them less priority). To that effect, the number of track features is
    /// counted (number of commas) and the package is downweighted
    /// by the number of track_features.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde_as(as = "OneOrMany<_>")]
    pub track_features: Vec<String>,

    /// The version of the package
    pub version: VersionWithSource,
}

impl PackageFile for IndexJson {
    fn package_path() -> &'static Path {
        Path::new("info/index.json")
    }

    fn from_str(str: &str) -> Result<Self, std::io::Error> {
        serde_json::from_str(str).map_err(Into::into)
    }
}

#[cfg(test)]
mod test {
    use super::{IndexJson, PackageFile};

    #[test]
    pub fn test_reconstruct_index_json() {
        let package_dir = tempfile::tempdir().unwrap();
        let package_path = tools::download_and_cache_file(
            "https://conda.anaconda.org/conda-forge/win-64/zlib-1.2.8-vc10_0.tar.bz2"
                .parse()
                .unwrap(),
            "ee9172dbe9ebd158e8e68d6d0f7dc2060f0c8230b44d2e9a3595b7cd7336b915",
        )
        .unwrap();
        rattler_package_streaming::fs::extract(&package_path, package_dir.path()).unwrap();

        insta::assert_yaml_snapshot!(IndexJson::from_package_directory(package_dir.path()).unwrap());
    }

    #[test]
    #[cfg(unix)]
    pub fn test_reconstruct_index_json_with_symlinks() {
        let package_dir = tempfile::tempdir().unwrap();

        let package_path = tools::download_and_cache_file(
            "https://conda.anaconda.org/conda-forge/linux-64/zlib-1.2.8-3.tar.bz2"
                .parse()
                .unwrap(),
            "85fcb6906b8686fe6341db89b4e6fc2631ad69ee6eab2f4823bfd64ae0b20ac8",
        )
        .unwrap();
        rattler_package_streaming::fs::extract(&package_path, package_dir.path()).unwrap();

        let package_dir = package_dir.into_path();
        println!("{}", package_dir.display());

        insta::assert_yaml_snapshot!(IndexJson::from_package_directory(&package_dir).unwrap());
    }
}
