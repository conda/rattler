//! Defines the `[RepoDataRecord]` struct.

use serde::{Deserialize, Serialize};
use url::Url;

use crate::{ChannelUrl, PackageRecord};

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
    pub file_name: String,

    /// The canonical URL from where to get this package.
    pub url: Url,

    /// The channel that contains the package. This might be `None` for a
    /// package that does not come from a channel.
    pub channel: Option<ChannelUrl>,
}

impl AsRef<PackageRecord> for RepoDataRecord {
    fn as_ref(&self) -> &PackageRecord {
        &self.package_record
    }
}
