use crate::PackageRecord;
use serde::{Deserialize, Serialize};
use url::Url;

/// Information about a package from repodata. It includes a [`crate::PackageRecord`] but it also stores
/// the source of the data (like the url and the channel).
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Ord, PartialOrd, Clone)]
pub struct RepoDataRecord {
    /// The data stored in the repodata.json.
    #[serde(flatten)]
    pub package_record: PackageRecord,

    /// The filename of the package
    #[serde(rename = "fn")]
    pub filename: String,

    /// The canonical URL from where to get this package.
    pub url: Url,

    /// String representation of the channel where the package comes from. This could be a URL but it
    /// could also be a channel name.
    pub channel: String,
}
