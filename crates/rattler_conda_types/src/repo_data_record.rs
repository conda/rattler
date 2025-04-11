//! Defines the `[RepoDataRecord]` struct.

use std::{collections::HashMap, vec::Vec};

use crate::{PackageName, PackageRecord};
use serde::{Deserialize, Serialize};
use url::Url;

/// Information about a package from repodata. It includes a [`crate::PackageRecord`] but it also stores
/// the source of the data (like the url and the channel).
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone, Hash)]
pub struct RepoDataRecord {
    /// The data stored in the repodata.json.
    #[serde(flatten)]
    pub package_record: PackageRecord,

    /// The filename of the package
    #[serde(rename = "fn")]
    pub file_name: String,

    /// The canonical URL from where to get this package.
    pub url: Url,

    /// String representation of the channel where the package comes from. This could be a URL but
    /// it could also be a channel name. Personally I would always add the complete URL here to be
    /// explicit about where the package came from.
    /// TODO: Refactor this into `Source` which can be a "name", "channelurl", or "direct url".
    pub channel: Option<String>,
}

impl AsRef<PackageRecord> for RepoDataRecord {
    fn as_ref(&self) -> &PackageRecord {
        &self.package_record
    }
}

/// Struct for the solver result containing records and their features
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct SolverResult {
    /// The records that are part of the solution to the solver task.
    pub records: Vec<RepoDataRecord>,
    /// The features of the records that are part of the solution to the solver task.
    pub features: HashMap<PackageName, Vec<String>>,
}
