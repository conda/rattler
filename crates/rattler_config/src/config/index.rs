//! Per-channel index configuration consumed by `rattler-index`.
//!
//! Mirrors the layout of [`crate::config::repodata_config::RepodataConfig`]:
//! a flat default block plus a per-channel map. Per-channel keys are full
//! channel URLs (`s3://my-bucket/my-channel`), URL prefixes
//! (`s3://my-bucket`), or absolute filesystem paths (`/srv/conda/internal`).
//! Longest matching prefix wins; values from less-specific entries are
//! layered as fallbacks.
//!
//! ```toml
//! [index-config]
//! write-zst = true
//! write-shards = true
//!
//! [index-config."s3://my-bucket"]
//! base-url = "../packages/"
//!
//! [index-config."s3://my-bucket/staging"]
//! write-shards = false
//! package-revision-assignment = "latest"
//!
//! [index-config."s3://my-bucket/staging".channel-relations]
//! base = "../conda-forge"
//!
//! [index-config."/srv/conda/internal"]
//! base-url = "../packages/"
//! ```
use std::{collections::HashMap, str::FromStr};

use rattler_conda_types::{ChannelRelations, RepodataRevision, RepodataRevisionInfo};
use serde::{Deserialize, Deserializer, Serialize, de::Error as DeError};

use crate::config::{Config, MergeError, ValidationError};

/// How packages are assigned to repodata revisions while indexing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageRevisionAssignment {
    /// Assign each package to the revision required by its `info/index.json`.
    /// Packages without an explicit `repodata_revision` are assigned to the
    /// oldest known revision that can represent their fields.
    #[default]
    FromIndexJson,
    /// Assign every package to the newest revision configured for the index.
    /// If no revisions are configured, packages are assigned to `Legacy`.
    Latest,
}

impl PackageRevisionAssignment {
    /// Pick the effective revision for a package given the latest configured
    /// revision for the channel.
    pub fn assign(
        self,
        package_revision: RepodataRevision,
        latest_revision: RepodataRevision,
    ) -> RepodataRevision {
        match self {
            Self::FromIndexJson => package_revision,
            Self::Latest => latest_revision,
        }
    }
}

/// Index options that apply to a single channel.
///
/// Every field is optional so that `IndexConfig` can layer multiple entries
/// without losing earlier values.
#[derive(Clone, Default, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct IndexChannelConfig {
    /// Whether to write `repodata.json.zst`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_zst: Option<bool>,

    /// Whether to write `repodata_shards.msgpack.zst` and shard files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_shards: Option<bool>,

    /// Repodata revisions to advertise in generated repodata.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_repodata_revisions"
    )]
    pub repodata_revisions: Option<Vec<RepodataRevisionInfo>>,

    /// How packages are assigned to repodata revisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_revision_assignment: Option<PackageRevisionAssignment>,

    /// `info.base_url` value written to generated repodata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// `info.channel_relations` value written to generated repodata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_relations: Option<ChannelRelations>,
}

impl IndexChannelConfig {
    /// Returns true if no fields are set.
    pub fn is_empty(&self) -> bool {
        self.write_zst.is_none()
            && self.write_shards.is_none()
            && self.repodata_revisions.is_none()
            && self.package_revision_assignment.is_none()
            && self.base_url.is_none()
            && self.channel_relations.is_none()
    }

    /// Layer `other` on top of `self`. Fields set in `other` win.
    pub fn merge(&self, other: Self) -> Self {
        Self {
            write_zst: other.write_zst.or(self.write_zst),
            write_shards: other.write_shards.or(self.write_shards),
            repodata_revisions: other
                .repodata_revisions
                .or_else(|| self.repodata_revisions.clone()),
            package_revision_assignment: other
                .package_revision_assignment
                .or(self.package_revision_assignment),
            base_url: other.base_url.or_else(|| self.base_url.clone()),
            channel_relations: other
                .channel_relations
                .or_else(|| self.channel_relations.clone()),
        }
    }
}

/// Index configuration with default options and per-channel overrides.
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct IndexConfig {
    /// Default options used when no `per_channel` entry matches.
    #[serde(flatten)]
    pub default: IndexChannelConfig,

    /// Per-channel overrides keyed by channel URL or absolute filesystem
    /// path. Longest matching prefix wins; less-specific entries are layered
    /// as fallbacks. Examples: `s3://my-bucket`,
    /// `s3://my-bucket/staging`, `/srv/conda/internal`.
    #[serde(flatten)]
    pub per_channel: HashMap<String, IndexChannelConfig>,
}

impl IndexConfig {
    /// Returns true if neither defaults nor per-channel entries are set.
    pub fn is_empty(&self) -> bool {
        self.default.is_empty() && self.per_channel.is_empty()
    }

    /// Resolve the effective options for a channel target.
    ///
    /// `target` is the canonical channel reference: a URL like
    /// `s3://my-bucket/staging` or an absolute filesystem path like
    /// `/srv/conda/internal`. All keys whose normalised form is a
    /// component-boundary prefix of `target` are layered onto `default`,
    /// shortest first, so the most specific match wins.
    pub fn resolve(&self, target: &str) -> IndexChannelConfig {
        let target = target.trim_end_matches('/');

        let mut matches: Vec<(&str, &IndexChannelConfig)> = self
            .per_channel
            .iter()
            .filter_map(|(key, cfg)| {
                let key_norm = key.trim_end_matches('/');
                is_prefix_match(key_norm, target).then_some((key_norm, cfg))
            })
            .collect();
        matches.sort_by_key(|(key, _)| key.len());

        let mut effective = self.default.clone();
        for (_, cfg) in matches {
            effective = effective.merge(cfg.clone());
        }
        effective
    }
}

/// True iff `key` equals `target` or is followed by a `/` separator in
/// `target`. Both inputs are expected to be slash-trimmed.
fn is_prefix_match(key: &str, target: &str) -> bool {
    if key == target {
        return true;
    }
    let Some(rest) = target.strip_prefix(key) else {
        return false;
    };
    rest.starts_with('/')
}

impl Config for IndexConfig {
    /// Merge another `IndexConfig` on top of this one.
    fn merge_config(self, other: &Self) -> Result<Self, MergeError> {
        let mut merged = self.clone();
        merged.default = merged.default.merge(other.default.clone());
        for (key, cfg) in &other.per_channel {
            merged
                .per_channel
                .entry(key.clone())
                .and_modify(|existing| *existing = existing.merge(cfg.clone()))
                .or_insert_with(|| cfg.clone());
        }
        Ok(merged)
    }

    fn validate(&self) -> Result<(), ValidationError> {
        for (key, cfg) in &self.per_channel {
            validate_channel_relations(key, cfg)?;
        }
        validate_channel_relations("default", &self.default)?;
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        Vec::new()
    }
}

fn validate_channel_relations(
    label: &str,
    cfg: &IndexChannelConfig,
) -> Result<(), ValidationError> {
    let Some(relations) = &cfg.channel_relations else {
        return Ok(());
    };
    if relations.base.is_some()
        && relations.overrides.is_some()
        && relations.base == relations.overrides
    {
        return Err(ValidationError::InvalidValue(
            format!("index-config.{label}.channel-relations"),
            "base and overrides must not be the same channel".to_string(),
        ));
    }
    Ok(())
}

fn deserialize_optional_repodata_revisions<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<RepodataRevisionInfo>>, D::Error>
where
    D: Deserializer<'de>,
{
    let revisions: Option<Vec<String>> = Option::deserialize(deserializer)?;
    revisions
        .map(|revs| {
            revs.into_iter()
                .map(|s| {
                    RepodataRevision::from_str(&s)
                        .map(|revision| RepodataRevisionInfo {
                            revision,
                            n_packages: None,
                            oldest: None,
                            newest: None,
                        })
                        .map_err(D::Error::custom)
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml: &str) -> IndexConfig {
        toml::from_str::<IndexConfig>(toml).unwrap()
    }

    #[test]
    fn parses_default_block() {
        let cfg = parse(
            r#"
write-zst = true
write-shards = false
repodata-revisions = ["v3"]
package-revision-assignment = "latest"
base-url = "../packages/"

[channel-relations]
base = "../conda-forge"
"#,
        );
        assert_eq!(cfg.default.write_zst, Some(true));
        assert_eq!(cfg.default.write_shards, Some(false));
        assert_eq!(cfg.default.repodata_revisions.as_ref().unwrap().len(), 1);
        assert_eq!(
            cfg.default.repodata_revisions.as_ref().unwrap()[0].revision,
            RepodataRevision::V3
        );
        assert_eq!(
            cfg.default.package_revision_assignment,
            Some(PackageRevisionAssignment::Latest)
        );
        assert_eq!(cfg.default.base_url.as_deref(), Some("../packages/"));
        assert_eq!(
            cfg.default
                .channel_relations
                .as_ref()
                .unwrap()
                .base
                .as_deref(),
            Some("../conda-forge")
        );
        assert!(cfg.per_channel.is_empty());
    }

    #[test]
    fn parses_per_channel_entries() {
        let cfg = parse(
            r#"
write-zst = true

["s3://my-bucket"]
base-url = "../packages/"

["s3://my-bucket/staging"]
write-shards = false
package-revision-assignment = "latest"

["/srv/conda/internal"]
base-url = "../local-packages/"
"#,
        );
        assert_eq!(cfg.default.write_zst, Some(true));
        assert_eq!(cfg.per_channel.len(), 3);
        assert_eq!(
            cfg.per_channel["s3://my-bucket"].base_url.as_deref(),
            Some("../packages/")
        );
        assert_eq!(
            cfg.per_channel["s3://my-bucket/staging"].write_shards,
            Some(false)
        );
        assert_eq!(
            cfg.per_channel["/srv/conda/internal"].base_url.as_deref(),
            Some("../local-packages/")
        );
    }

    #[test]
    fn resolves_longest_prefix_match() {
        let cfg = parse(
            r#"
write-zst = true
write-shards = true

["s3://my-bucket"]
base-url = "../packages/"
package-revision-assignment = "from-index-json"

["s3://my-bucket/staging"]
package-revision-assignment = "latest"
write-shards = false
"#,
        );
        let resolved = cfg.resolve("s3://my-bucket/staging");
        assert_eq!(resolved.write_zst, Some(true));
        assert_eq!(resolved.write_shards, Some(false)); // staging wins
        assert_eq!(resolved.base_url.as_deref(), Some("../packages/")); // host fallback
        assert_eq!(
            resolved.package_revision_assignment,
            Some(PackageRevisionAssignment::Latest) // staging wins
        );
    }

    #[test]
    fn resolve_falls_back_to_default_for_unknown_target() {
        let cfg = parse(
            r#"
write-zst = false

["s3://my-bucket"]
base-url = "../packages/"
"#,
        );
        let resolved = cfg.resolve("s3://other-bucket/channel");
        assert_eq!(resolved.write_zst, Some(false));
        assert!(resolved.base_url.is_none());
    }

    #[test]
    fn resolve_does_not_match_partial_component() {
        let cfg = parse(
            r#"
["s3://my-bucket"]
base-url = "../packages/"
"#,
        );
        let resolved = cfg.resolve("s3://my-bucket-other/channel");
        assert!(resolved.base_url.is_none());
    }

    #[test]
    fn rejects_numeric_repodata_revisions() {
        let err = toml::from_str::<IndexConfig>("repodata-revisions = [3]\n").unwrap_err();
        assert!(
            err.to_string().contains("invalid type"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validates_matching_base_and_overrides() {
        let cfg = parse(
            r#"
[channel-relations]
base = "../same"
overrides = "../same"
"#,
        );
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("must not be the same channel"));
    }

    #[test]
    fn merge_layers_per_channel_entries() {
        let base = parse(
            r#"
write-zst = true

["s3://my-bucket"]
base-url = "../packages/"
"#,
        );
        let other = parse(
            r#"
write-shards = false

["s3://my-bucket"]
package-revision-assignment = "latest"
"#,
        );
        let merged = base.merge_config(&other).unwrap();
        assert_eq!(merged.default.write_zst, Some(true));
        assert_eq!(merged.default.write_shards, Some(false));
        let entry = &merged.per_channel["s3://my-bucket"];
        assert_eq!(entry.base_url.as_deref(), Some("../packages/"));
        assert_eq!(
            entry.package_revision_assignment,
            Some(PackageRevisionAssignment::Latest)
        );
    }
}
