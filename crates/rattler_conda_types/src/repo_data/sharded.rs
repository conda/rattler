//! Structs to deal with repodata "shards" which are per-package repodata files.

use crate::package::DistArchiveIdentifier;
use crate::repo_data::{ChannelRelations, ExperimentalV3Packages, RepodataRevisionInfo};
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

    /// Repodata revisions available through this sharded index.
    ///
    /// See <https://github.com/conda/ceps/pull/146>.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repodata_revisions: Vec<RepodataRevisionInfo>,

    /// Optional relationships to other channels as defined in
    /// [CEP-42](https://github.com/conda/ceps/blob/main/cep-0042.md).
    #[serde(default, skip_serializing_if = "ChannelRelations::is_none_or_empty")]
    pub channel_relations: Option<ChannelRelations>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // See https://github.com/conda/ceps/blob/main/cep-0042.md
    #[test]
    fn test_sharded_subdir_info_channel_relations() {
        // Deserialize a sharded index with channel_relations.
        let raw = r#"{
            "subdir": "linux-64",
            "base_url": "./",
            "shards_base_url": "./shards/",
            "channel_relations": {
                "base": "../conda-forge"
            }
        }"#;
        let info: ShardedSubdirInfo = serde_json::from_str(raw).unwrap();
        let relations = info.channel_relations.as_ref().unwrap();
        assert_eq!(relations.base.as_deref(), Some("../conda-forge"));
        assert_eq!(relations.overrides, None);

        // `channel_relations` must be omitted when it is `None` and when all
        // of its fields are unset.
        for channel_relations in [None, Some(ChannelRelations::default())] {
            let info = ShardedSubdirInfo {
                subdir: "linux-64".to_string(),
                base_url: "./".to_string(),
                shards_base_url: "./shards/".to_string(),
                created_at: None,
                repodata_revisions: Vec::new(),
                channel_relations,
            };
            let json = serde_json::to_string(&info).unwrap();
            assert!(!json.contains("channel_relations"));
        }
    }
}

/// An individual shard that contains repodata for a single package name.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Shard {
    /// The records for all `.tar.bz2` packages
    pub packages: IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState>,

    /// The records for all `.conda` packages
    #[serde(rename = "packages.conda", default)]
    pub conda_packages: IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState>,

    /// Packages stored under the `v3` top-level key.
    #[serde(
        default,
        rename = "v3",
        skip_serializing_if = "ExperimentalV3Packages::is_empty"
    )]
    pub experimental_v3: ExperimentalV3Packages,

    /// The file names of all removed for this shard
    #[serde(default)]
    pub removed: ahash::HashSet<DistArchiveIdentifier>,
}
