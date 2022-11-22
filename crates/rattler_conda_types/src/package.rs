//! Contains models of files that are found in the `info/` directory of a package.

use crate::{
    utils::serde::{LossyUrl, MultiLineString, VecSkipNone},
    RunExports, Version,
};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none, DisplayFromStr, OneOrMany, Same};
use std::collections::HashMap;
use url::Url;

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

#[serde_as]
#[skip_serializing_none]
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct About {
    /// Description of the package
    #[serde_as(deserialize_as = "Option<MultiLineString>")]
    pub description: Option<String>,

    /// Short summary description
    #[serde_as(deserialize_as = "Option<MultiLineString>")]
    pub summary: Option<String>,

    /// Optionally, the license
    pub license: Option<String>,

    /// Optionally, the license family
    pub license_family: Option<String>,

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

    /// URL to the latest source code of the package
    #[serde(default)]
    #[serde_as(deserialize_as = "LossyUrl")]
    pub source_url: Option<Url>,

    /// A list of channels that where used during the build
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub channels: Vec<String>,
}
