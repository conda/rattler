//! Structs to deal with repodata "shards" which are per-package repodata files.

use crate::package::DistArchiveIdentifier;
use crate::repo_data::WhlPackageRecord;
use crate::PackageRecord;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use rattler_digest::{serde::SerializableHash, Sha256, Sha256Hash};
use serde::{Deserialize, Serialize};

/// The sharded repodata holds a hashmap of package name -> shard (hash).
/// This index file is stored under
/// `<channel>/<subdir>/repodata_shards.msgpack.zst`
#[serde_with::serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardedRepodata {
    /// Additional information about the sharded subdirectory such as the base
    /// url.
    pub info: ShardedSubdirInfo,
    /// The individual shards indexed by package name.
    #[serde_as(as = "ahash::HashMap<_, SerializableHash<Sha256>>")]
    pub shards: ahash::HashMap<String, Sha256Hash>,
}

/// Information about a sharded subdirectory that is stored inside the index
/// file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardedSubdirInfo {
    /// The name of the subdirectory
    pub subdir: String,

    /// The base url of the subdirectory. This is the location where the actual
    /// packages are stored.
    ///
    /// This is used to construct the full url of the packages.
    pub base_url: String,

    /// The base url of the individual shards. This is the location where the
    /// actual packages are stored.
    ///
    /// This is used to construct the full url of the shard.
    pub shards_base_url: String,

    /// The date at which this entry was created.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
}

/// An individual shard that contains repodata for a single package name.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Shard {
    /// The records for all `.tar.bz2` packages
    pub packages: IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState>,

    /// The records for all `.conda` packages
    #[serde(rename = "packages.conda", default)]
    pub conda_packages: IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState>,

    /// The records for all `.whl` packages
    #[serde(rename = "packages.whl", default)]
    pub experimental_whl_packages:
        IndexMap<DistArchiveIdentifier, WhlPackageRecord, ahash::RandomState>,

    /// The file names of all removed for this shard
    #[serde(default)]
    pub removed: ahash::HashSet<DistArchiveIdentifier>,
}
