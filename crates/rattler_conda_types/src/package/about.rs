use std::{io::Error, path::Path};

use crate::{
    package::PackageFile,
    utils::serde::{LossyUrl, MultiLineString, VecSkipNone},
};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none, OneOrMany, Same};

use url::Url;

use rattler_macros::sorted;

/// The `about.json` file contains metadata about the package
#[serde_as]
#[sorted]
#[skip_serializing_none]
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct AboutJson {
    /// A list of channels that where used during the build
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub channels: Vec<String>,

    /// Description of the package
    #[serde_as(deserialize_as = "Option<MultiLineString>")]
    pub description: Option<String>,

    /// URL to the development page of the package
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    #[serde_as(
        deserialize_as = "VecSkipNone<OneOrMany<LossyUrl>>",
        serialize_as = "OneOrMany<Same>"
    )]
    pub dev_url: Vec<Url>,

    /// URL to the documentation of the package
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    #[serde_as(
        deserialize_as = "VecSkipNone<OneOrMany<LossyUrl>>",
        serialize_as = "OneOrMany<Same>"
    )]
    pub doc_url: Vec<Url>,

    /// URL to the homepage of the package
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    #[serde_as(
        deserialize_as = "VecSkipNone<OneOrMany<LossyUrl>>",
        serialize_as = "OneOrMany<Same>"
    )]
    pub home: Vec<Url>,

    /// Optionally, the license
    pub license: Option<String>,

    /// Optionally, the license family
    pub license_family: Option<String>,

    /// URL to the latest source code of the package
    #[serde(default)]
    #[serde_as(deserialize_as = "LossyUrl")]
    pub source_url: Option<Url>,

    /// Short summary description
    #[serde_as(deserialize_as = "Option<MultiLineString>")]
    pub summary: Option<String>,
}

impl PackageFile for AboutJson {
    fn package_path() -> &'static Path {
        Path::new("info/about.json")
    }

    fn from_str(str: &str) -> Result<Self, Error> {
        serde_json::from_str(str).map_err(Into::into)
    }
}

#[cfg(test)]
mod test {
    use super::{AboutJson, PackageFile};

    #[test]
    pub fn test_reconstruct_about_json() {
        let package_dir = tempfile::tempdir().unwrap();
        rattler_package_streaming::fs::extract(
            &crate::get_test_data_dir().join("conda-22.11.1-py38haa244fe_1.conda"),
            package_dir.path(),
        )
        .unwrap();

        insta::assert_yaml_snapshot!(AboutJson::from_package_directory(package_dir.path()).unwrap());
    }

    #[test]
    pub fn test_reconstruct_about_json_mamba() {
        let package_dir = tempfile::tempdir().unwrap();
        rattler_package_streaming::fs::extract(
            &crate::get_test_data_dir().join("mamba-1.0.0-py38hecfeebb_2.tar.bz2"),
            package_dir.path(),
        )
        .unwrap();

        let package_dir = package_dir.into_path();
        println!("{}", package_dir.display());

        insta::assert_yaml_snapshot!(AboutJson::from_package_directory(&package_dir).unwrap());
    }
}
