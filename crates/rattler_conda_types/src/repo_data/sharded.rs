//! Structs to deal with repodata "shards" which are per-package repodata files.

use crate::PackageRecord;
use crate::package::DistArchiveIdentifier;
#[cfg(feature = "experimental-virtual-package-plugins")]
use crate::repo_data::VirtualPackagePlugins;
use crate::repo_data::{ChannelRelations, RepodataRevisions, V3Packages};
#[cfg(feature = "experimental-virtual-package-plugins")]
use crate::utils::serde::DeserializeFromStrUnchecked;
use crate::utils::serde::{sort_index_map_alphabetically, sort_set_alphabetically};
use indexmap::IndexMap;
use jiff::Timestamp;
use rattler_digest::{Sha256, Sha256Hash, serde::SerializableHash};
use serde::{Deserialize, Serialize};
use serde_with::{DisplayFromStr, serde_as};

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
#[serde_as]
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
    pub created_at: Option<Timestamp>,

    /// Repodata revisions available through this sharded index.
    ///
    /// Serialized as a `vN`-keyed dictionary per the CEP draft
    /// <https://github.com/conda/ceps/pull/146>.
    #[serde_as(as = "IndexMap<DisplayFromStr, _>")]
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub repodata_revisions: RepodataRevisions,

    /// Optional relationships to other channels as defined in
    /// [CEP-42](https://github.com/conda/ceps/blob/main/cep-0042.md).
    #[serde(default, skip_serializing_if = "ChannelRelations::is_none_or_empty")]
    pub channel_relations: Option<ChannelRelations>,

    /// Virtual package detection plugins registered by the channel.
    #[cfg(feature = "experimental-virtual-package-plugins")]
    #[serde_as(
        deserialize_as = "IndexMap<DeserializeFromStrUnchecked, Vec<DeserializeFromStrUnchecked>>"
    )]
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub virtual_package_plugins: VirtualPackagePlugins,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PackageName, Version};

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ShardedIndexShape {
        info: serde::de::IgnoredAny,
        shards: serde::de::IgnoredAny,
    }

    /// Shards are content-addressed (stored under the hash of their bytes), so
    /// serialization must not depend on the insertion order of the underlying
    /// maps and sets — otherwise every producer run writes a fresh shard file
    /// and orphans the previous one.
    #[test]
    fn test_shard_serialization_is_insertion_order_independent() {
        let entry = |n: u64| {
            let key =
                DistArchiveIdentifier::try_from_filename(&format!("multi-1.0.0-h_{n}.tar.bz2"))
                    .unwrap();
            let record = PackageRecord::new(
                PackageName::new_unchecked("multi"),
                Version::major(1),
                format!("h_{n}"),
            );
            (key, record)
        };

        let shard_with_order = |order: &mut dyn Iterator<Item = u64>| {
            let mut shard = Shard::default();
            for n in order {
                let (key, record) = entry(n);
                shard.conda_packages.insert(key.clone(), record.clone());
                shard.packages.insert(key.clone(), record);
                shard.removed.insert(key);
            }
            rmp_serde::to_vec_named(&shard).unwrap()
        };

        let ascending = shard_with_order(&mut (0..10));
        let descending = shard_with_order(&mut (0..10).rev());
        assert_eq!(
            ascending, descending,
            "shard bytes must be independent of insertion order"
        );
    }

    #[test]
    fn test_sharded_repodata_without_v3_roundtrips_through_named_msgpack() {
        let sharded_repodata = ShardedRepodata {
            info: ShardedSubdirInfo {
                subdir: "linux-64".to_string(),
                base_url: "./".to_string(),
                shards_base_url: "./shards/".to_string(),
                created_at: None,
                repodata_revisions: IndexMap::default(),
                channel_relations: None,
                #[cfg(feature = "experimental-virtual-package-plugins")]
                virtual_package_plugins: VirtualPackagePlugins::default(),
            },
            shards: ahash::HashMap::default(),
        };

        let encoded = rmp_serde::to_vec_named(&sharded_repodata).unwrap();
        let shape: ShardedIndexShape = rmp_serde::from_slice(&encoded).unwrap();
        let _ = (shape.info, shape.shards);

        let decoded: ShardedRepodata = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(decoded.info.subdir, "linux-64");
    }

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
                repodata_revisions: IndexMap::default(),
                channel_relations,
                #[cfg(feature = "experimental-virtual-package-plugins")]
                virtual_package_plugins: VirtualPackagePlugins::default(),
            };
            let json = serde_json::to_string(&info).unwrap();
            assert!(!json.contains("channel_relations"));
        }
    }

    #[cfg(feature = "experimental-virtual-package-plugins")]
    #[test]
    fn test_sharded_subdir_info_virtual_package_plugins() {
        let raw = r#"{
            "subdir": "linux-64",
            "base_url": "./",
            "shards_base_url": "./shards/",
            "virtual_package_plugins": {
                "cuda-detect": ["__cuda", "__cuda_arch"]
            }
        }"#;
        let info: ShardedSubdirInfo = serde_json::from_str(raw).unwrap();
        assert_eq!(
            info.virtual_package_plugins[&PackageName::new_unchecked("cuda-detect")]
                .iter()
                .map(PackageName::as_source)
                .collect::<Vec<_>>(),
            ["__cuda", "__cuda_arch"]
        );

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"virtual_package_plugins\""));

        // Omitted entirely when no plugins are registered.
        let info = ShardedSubdirInfo {
            virtual_package_plugins: VirtualPackagePlugins::default(),
            ..info
        };
        assert!(
            !serde_json::to_string(&info)
                .unwrap()
                .contains("virtual_package_plugins")
        );
    }
}

/// An individual shard that contains repodata for a single package name.
///
/// Shards are content-addressed by the hash of their serialized bytes, so all
/// maps and sets are sorted during serialization to keep the output
/// deterministic regardless of insertion order.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Shard {
    /// The records for all `.tar.bz2` packages
    #[serde(serialize_with = "sort_index_map_alphabetically")]
    pub packages: IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState>,

    /// The records for all `.conda` packages
    #[serde(
        rename = "packages.conda",
        default,
        serialize_with = "sort_index_map_alphabetically"
    )]
    pub conda_packages: IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState>,

    /// Packages stored under the `v3` top-level key.
    #[serde(default, skip_serializing_if = "V3Packages::is_empty")]
    pub v3: V3Packages,

    /// The file names of all removed for this shard
    #[serde(default, serialize_with = "sort_set_alphabetically")]
    pub removed: ahash::HashSet<DistArchiveIdentifier>,
}
