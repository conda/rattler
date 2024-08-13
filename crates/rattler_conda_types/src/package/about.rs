use std::collections::BTreeMap;
use std::{io::Error, path::Path};

use crate::{
    package::PackageFile,
    utils::serde::{LossyUrl, MultiLineString, VecSkipNone},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_with::{serde_as, skip_serializing_none, OneOrMany, Same};

use url::Url;

use rattler_macros::sorted;

/// The `about.json` file contains metadata about the package
#[serde_as]
#[sorted]
#[skip_serializing_none]
#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
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

    /// Extra metadata that was passed during the build
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub extra: BTreeMap<String, Value>,

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

    use std::collections::BTreeMap;

    use insta::assert_snapshot;
    use serde_json::json;
    use url::Url;

    use super::{AboutJson, PackageFile};

    #[test]
    pub fn test_reconstruct_about_json() {
        let package_dir = tempfile::tempdir().unwrap();

        let package_path = tools::download_and_cache_file(
            "https://conda.anaconda.org/conda-forge/win-64/conda-22.11.1-py38haa244fe_1.conda"
                .parse()
                .unwrap(),
            "a8a44c5ff2b2f423546d49721ba2e3e632233c74a813c944adf8e5742834930e",
        )
        .unwrap();
        rattler_package_streaming::fs::extract(&package_path, package_dir.path()).unwrap();

        insta::assert_yaml_snapshot!(AboutJson::from_package_directory(package_dir.path()).unwrap());
    }

    #[test]
    pub fn test_reconstruct_about_json_mamba() {
        let package_dir = tempfile::tempdir().unwrap();

        let package_path = tools::download_and_cache_file(
            "https://conda.anaconda.org/conda-forge/win-64/mamba-1.0.0-py38hecfeebb_2.tar.bz2"
                .parse()
                .unwrap(),
            "f44c4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97",
        )
        .unwrap();
        rattler_package_streaming::fs::extract(&package_path, package_dir.path()).unwrap();

        let package_dir = package_dir.into_path();
        println!("{}", package_dir.display());

        insta::assert_yaml_snapshot!(AboutJson::from_package_directory(&package_dir).unwrap());
    }

    #[test]
    fn test_extra_field_is_recorded_when_present() {
        // Define a sample AboutJson instance with extra field populated
        let mut extra_metadata = BTreeMap::default();
        extra_metadata.insert("flow_id".to_string(), json!("2024.08.13".to_string()));
        extra_metadata.insert("some_values".to_string(), json!({ "an": "object" }));

        let about = AboutJson {
            channels: vec!["conda-forge".to_string()],
            description: Some("A sample package".to_string()),
            dev_url: vec![],
            doc_url: vec![],
            extra: extra_metadata.clone(),
            home: vec![],
            license: Some("MIT".to_string()),
            license_family: Some("MIT".to_string()),
            source_url: Some(Url::parse("https://github.com/some-user/sample").unwrap()),
            summary: Some("This is a test package".to_string()),
        };

        // Serialize the AboutJson instance to JSON
        let serialized = serde_json::to_string(&about).expect("Serialization failed");

        // Deserialize the JSON back to an AboutJson instance
        let deserialized: AboutJson =
            serde_json::from_str(&serialized).expect("Deserialization failed");

        // Verify that the deserialized instance matches the original
        assert_snapshot!(serialized);
        assert_eq!(about, deserialized);
    }

    #[test]
    fn test_extra_field_is_skipped() {
        // Define a sample AboutJson instance with extra field populated
        let about = AboutJson {
            channels: vec!["conda-forge".to_string()],
            description: Some("A sample package".to_string()),
            dev_url: vec![],
            doc_url: vec![],
            extra: BTreeMap::default(),
            home: vec![],
            license: Some("MIT".to_string()),
            license_family: Some("MIT".to_string()),
            source_url: Some(Url::parse("https://github.com/some-user/sample").unwrap()),
            summary: Some("This is a test package".to_string()),
        };

        // Serialize the AboutJson instance to JSON
        let serialized = serde_json::to_string(&about).expect("Serialization failed");

        // Verify that the deserialized instance matches the original
        assert_snapshot!(serialized);
    }
}
