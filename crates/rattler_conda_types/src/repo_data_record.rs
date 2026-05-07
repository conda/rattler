//! Defines the `[RepoDataRecord]` struct.

use std::{collections::HashMap, str::FromStr, vec::Vec};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::{package::DistArchiveIdentifier, PackageName, PackageRecord, Platform};

/// Information about a package from repodata. It includes a
/// [`crate::PackageRecord`] but it also stores the source of the data (like the
/// url and the channel).
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone, Hash)]
pub struct RepoDataRecord {
    /// The data stored in the repodata.json.
    #[serde(flatten)]
    pub package_record: PackageRecord,

    /// The filename of the package
    #[serde(rename = "fn")]
    pub identifier: DistArchiveIdentifier,

    /// The canonical URL from where to get this package.
    pub url: Url,

    /// String representation of the channel where the package comes from. This
    /// could be a URL but it could also be a channel name. Personally I
    /// would always add the complete URL here to be explicit about where
    /// the package came from. TODO: Refactor this into `Source` which can
    /// be a "name", "channelurl", or "direct url".
    pub channel: Option<String>,
}

impl PartialOrd for RepoDataRecord {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RepoDataRecord {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.package_record.cmp(&other.package_record)
    }
}

impl RepoDataRecord {
    /// Returns true if `run_exports` is some.
    pub fn has_run_exports(&self) -> bool {
        self.package_record.has_run_exports()
    }

    /// Returns URL of package channel subdir.
    pub fn platform_url(&self) -> Url {
        let mut url = self.url.clone();
        if let Some(segments) = url.path_segments() {
            let mut out = Vec::new();
            for segment in segments {
                out.push(segment);
                if Platform::from_str(segment).is_ok() {
                    break;
                }
            }

            let path = out.join("/");

            // Ensure the path ends with a trailing slash. This is important for when we
            // join this URL later.
            if path.ends_with("/") {
                url.set_path(&path);
            } else {
                url.set_path(&format!("{path}/"));
            }
        }

        url
    }
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
    /// The features of the records that are part of the solution to the solver
    /// task.
    pub extras: HashMap<PackageName, Vec<String>>,
}
