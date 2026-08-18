//! Structs to deal with repodata "shards" which are per-package repodata files.

use crate::PackageRecord;
use crate::package::DistArchiveIdentifier;
#[cfg(feature = "experimental-virtual-package-plugins")]
use crate::repo_data::VirtualPackagePlugins;
use crate::repo_data::{ChannelRelations, RepodataRevisions, V3Packages};
#[cfg(feature = "experimental-virtual-package-plugins")]
use crate::utils::serde::DeserializeVirtualPackagePlugins;
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
    #[serde_as(deserialize_as = "DeserializeVirtualPackagePlugins")]
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
    fn test_sharded_subdir_info_normalizes_plugin_names() {
        let raw = r#"{
            "subdir": "linux-64",
            "base_url": "./",
            "shards_base_url": "./shards/",
            "virtual_package_plugins": { "CUDA-Detect": ["__cuda"] }
        }"#;
        let info: ShardedSubdirInfo = serde_json::from_str(raw).unwrap();
        let plugin = PackageName::try_from("cuda-detect").unwrap();
        assert!(
            info.virtual_package_plugins.contains_key(&plugin),
            "a sharded registration is unreachable by its normalized name"
        );
        assert!(
            info.virtual_package_plugins[&plugin]
                .contains(&PackageName::try_from("__cuda").unwrap()),
            "a provided virtual package never matches the detected name"
        );
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

    /// A msgpack `ShardedSubdirInfo` document with the given already-encoded
    /// msgpack bytes as the value of `virtual_package_plugins`. msgpack can
    /// express shapes JSON cannot (binary data, non-string keys, invalid
    /// UTF-8, ext types), and the shard index is remote data, so those shapes
    /// are reachable in production.
    #[cfg(feature = "experimental-virtual-package-plugins")]
    fn msgpack_info_with_plugins_section(section: &[u8]) -> Vec<u8> {
        let fixstr = |out: &mut Vec<u8>, text: &str| {
            out.push(0xa0 | u8::try_from(text.len()).unwrap());
            out.extend_from_slice(text.as_bytes());
        };
        let mut out = vec![0x84];
        fixstr(&mut out, "subdir");
        fixstr(&mut out, "linux-64");
        fixstr(&mut out, "base_url");
        fixstr(&mut out, "./");
        fixstr(&mut out, "shards_base_url");
        fixstr(&mut out, "./shards/");
        fixstr(&mut out, "virtual_package_plugins");
        out.extend_from_slice(section);
        out
    }

    #[cfg(feature = "experimental-virtual-package-plugins")]
    #[track_caller]
    fn parse_msgpack_plugins_section(section: &[u8]) -> ShardedSubdirInfo {
        rmp_serde::from_slice(&msgpack_info_with_plugins_section(section)).unwrap_or_else(|err| {
            panic!("the malformed section rejected the surrounding shard index: {err}")
        })
    }

    #[cfg(feature = "experimental-virtual-package-plugins")]
    #[track_caller]
    fn assert_msgpack_plugins_section_dropped(section: &[u8]) {
        let info = parse_msgpack_plugins_section(section);
        assert!(
            info.virtual_package_plugins.is_empty(),
            "expected the whole section to be dropped, got {:?}",
            info.virtual_package_plugins
        );
        assert_eq!(
            info.subdir, "linux-64",
            "the rest of the index info was lost"
        );
    }

    /// A registration key that is not a string cannot be a package name, so
    /// the section is not a map of registrations: an error that drops the
    /// section -- and never the surrounding shard index, which is remote data
    /// one hostile key must not be able to take down.
    #[cfg(feature = "experimental-virtual-package-plugins")]
    #[test]
    fn test_sharded_subdir_info_drops_the_section_on_a_non_string_key() {
        let value = [&[0x91, 0xa3][..], b"__x"].concat();
        for (label, key) in [
            ("an integer key", vec![0x05]),
            ("a nil key", vec![0xc0]),
            ("a binary key", [&[0xc4, 0x02][..], b"ab"].concat()),
            ("an invalid UTF-8 str key", vec![0xa2, 0xff, 0xfe]),
        ] {
            let section = [&[0x81][..], &key, &value].concat();
            let info = parse_msgpack_plugins_section(&section);
            assert!(
                info.virtual_package_plugins.is_empty(),
                "{label} did not drop the section, got {:?}",
                info.virtual_package_plugins
            );
        }
    }

    #[cfg(feature = "experimental-virtual-package-plugins")]
    #[test]
    fn test_sharded_subdir_info_drops_the_section_on_an_ext_value() {
        // fixext1, type 1, one payload byte: not a map of registrations.
        assert_msgpack_plugins_section_dropped(&[0xd4, 0x01, 0x2a]);
    }

    #[cfg(feature = "experimental-virtual-package-plugins")]
    #[test]
    fn test_sharded_subdir_info_drops_a_binary_element_alone() {
        // {"cuda-detect": ["__cuda", bin"__x"]}: binary data is not a name,
        // however name-like its bytes, and must be dropped like any other
        // shape that is not a string.
        let section = [
            &[0x81, 0xab][..],
            b"cuda-detect",
            &[0x92, 0xa6],
            b"__cuda",
            &[0xc4, 0x03],
            b"__x",
        ]
        .concat();
        let info = parse_msgpack_plugins_section(&section);
        assert_eq!(
            info.virtual_package_plugins[&PackageName::new_unchecked("cuda-detect")]
                .iter()
                .map(PackageName::as_source)
                .collect::<Vec<_>>(),
            ["__cuda"],
            "a binary element was not dropped alone"
        );
    }

    /// The shard index travels as msgpack, and the registration deserializer
    /// relies on `deserialize_any` and an untagged enum, both of which a
    /// self-describing format must support.
    #[cfg(feature = "experimental-virtual-package-plugins")]
    #[test]
    fn test_sharded_subdir_info_plugins_survive_msgpack() {
        let raw = r#"{
            "subdir": "linux-64",
            "base_url": "./",
            "shards_base_url": "./shards/",
            "virtual_package_plugins": { "cuda-detect": ["__cuda", "__cuda_arch"] }
        }"#;
        let info: ShardedSubdirInfo = serde_json::from_str(raw).unwrap();
        let bytes = rmp_serde::to_vec_named(&info).unwrap();
        let back: ShardedSubdirInfo = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(
            back.virtual_package_plugins, info.virtual_package_plugins,
            "the registrations changed through a msgpack round trip"
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
