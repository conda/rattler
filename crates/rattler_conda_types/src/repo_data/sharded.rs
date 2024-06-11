//! Structs to deal with repodata "shards" which are per-package repodata files.
use fxhash::{FxHashMap, FxHashSet};
use rattler_digest::Sha256Hash;
use serde::{Deserialize, Serialize};

use crate::PackageRecord;

/// The sharded repodata holds a hashmap of package name -> shard (hash).
/// This index file is stored under `<channel>/<subdir>/repodata_shards.msgpack.zst`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardedRepodata {
    /// Additional information about the sharded subdirectory such as the base url.
    pub info: ShardedSubdirInfo,
    /// The individual shards indexed by package name.
    pub shards: FxHashMap<String, Sha256Hash>,
}

/// Information about a sharded subdirectory that is stored inside the index file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardedSubdirInfo {
    /// The name of the subdirectory
    pub subdir: String,

    /// The base url of the subdirectory. This is the location where the actual
    /// packages are stored.
    ///
    /// This is used to construct the full url of the packages.
    pub base_url: String,

    /// The base url of the individual shards. This is the location where the actual
    /// packages are stored.
    ///
    /// This is used to construct the full url of the shard.
    pub shards_base_url: String,
}

/// An individual shard that contains repodata for a single package name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shard {
    /// The records for all `.tar.bz2` packages
    pub packages: FxHashMap<String, PackageRecord>,

    /// The records for all `.conda` packages
    #[serde(rename = "packages.conda", default)]
    pub conda_packages: FxHashMap<String, PackageRecord>,

    /// The file names of all removed for this shard
    #[serde(default)]
    pub removed: FxHashSet<String>,
}
