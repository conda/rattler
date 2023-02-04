//! Contains models of files that are found in the `info/` directory of a package.

use crate::{
    utils::serde::{LossyUrl, MultiLineString, VecSkipNone},
    RunExports, Version,
};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none, DisplayFromStr, OneOrMany, Same};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::str::FromStr;
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

/// A representation of the `paths.json` file found in package archives.
///
/// The `paths.json` file contains information about every file included with the package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathsJson {
    /// The version of the file
    pub paths_version: usize,

    /// All entries included in the package.
    pub paths: Vec<PathsEntry>,
}

impl PathsJson {
    /// Parses a `paths.json` file from a reader.
    pub fn from_reader(mut reader: impl Read) -> Result<Self, std::io::Error> {
        let mut str = String::new();
        reader.read_to_string(&mut str)?;
        Self::from_str(&str)
    }

    /// Parses a `paths.json` file from a file.
    pub fn from_path(path: &Path) -> Result<Self, std::io::Error> {
        Self::from_reader(File::open(path)?)
    }
}

impl FromStr for PathsJson {
    type Err = std::io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s).map_err(Into::into)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct PathsEntry {
    /// The relative path from the root of the package
    #[serde(rename = "_path")]
    pub relative_path: PathBuf,

    /// Determines how to include the file when installing the package
    pub path_type: PathType,

    /// The type of the file, either binary or text.
    #[serde(default, skip_serializing_if = "FileMode::is_binary")]
    pub file_mode: FileMode,

    /// Optionally the placeholder prefix used in the file. If this value is `None` the prefix is not
    /// present in the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_placeholder: Option<String>,

    /// Whether or not this file should be linked or not when installing the package.
    #[serde(
        default = "no_link_default",
        skip_serializing_if = "is_no_link_default"
    )]
    pub no_link: bool,

    /// A hex representation of the SHA256 hash of the contents of the file.
    /// This entry is only present in version 1 of the paths.json file.
    pub sha256: Option<String>,

    /// The size of the file in bytes
    /// This entry is only present in version 1 of the paths.json file.
    pub size_in_bytes: Option<u64>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum FileMode {
    Binary,
    Text,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum PathType {
    HardLink,
    SoftLink,
    Directory,
}

impl Default for FileMode {
    fn default() -> Self {
        FileMode::Binary
    }
}

impl FileMode {
    /// Returns `true` if the file type is a binary file.
    pub fn is_binary(&self) -> bool {
        matches!(self, FileMode::Binary)
    }
}

/// Returns the default value for the "no_link" value of a [`PathsEntry`]
fn no_link_default() -> bool {
    false
}

/// Returns true if the value is equal to the default value for the "no_link" value of a [`PathsEntry`]
fn is_no_link_default(value: &bool) -> bool {
    *value == no_link_default()
}
