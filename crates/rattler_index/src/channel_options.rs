//! TOML configuration for channel indexing options.
//!
//! [`ChannelOptions`] is the file format consumed by
//! `rattler-index --channel-options`. It lets users keep channel metadata and
//! repodata revision settings in a small TOML file instead of spelling out every
//! option on the command line.
//!
//! Command line flags take precedence over values from the TOML file. Omitted
//! fields fall back to the normal `rattler-index` defaults.
//!
//! # Example
//!
//! ```toml
//! write-zst = true
//! write-shards = true
//! repodata-revisions = ["v3"]
//! package-revision-assignment = "latest"
//! base-url = "../packages/"
//!
//! [channel-relations]
//! base = "../conda-forge"
//! overrides = "../fallback"
//! ```

use std::{fmt, path::Path, str::FromStr};

use anyhow::Context;
use rattler_conda_types::{ChannelRelations, RepodataRevision, RepodataRevisionInfo};
use serde::{
    de::{Error as DeError, Visitor},
    Deserialize, Deserializer,
};

use crate::PackageRevisionAssignment;

/// Metadata written into generated channel repodata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChannelMetadata {
    /// The `info.base_url` value written to `repodata.json`.
    ///
    /// This can be an absolute URL, an absolute path, or a relative URL.
    pub base_url: Option<String>,
    /// The `info.channel_relations` value written to `repodata.json`.
    pub channel_relations: Option<ChannelRelations>,
}

/// TOML channel options consumed by the indexer.
///
/// The format uses kebab-case keys. Snake-case aliases are accepted for
/// compatibility with the Rust field names, but new files should use
/// kebab-case.
///
/// `base-url` and `channel-relations` are written to the `info` object in each
/// generated `repodata.json` file and to the sharded repodata metadata when
/// shards are enabled. `channel-relations.base` and
/// `channel-relations.overrides` are each a single channel reference, matching
/// CEP-42.
///
/// For example:
///
/// ```toml
/// write-zst = true
/// write-shards = true
/// repodata-revisions = ["v3"]
/// package-revision-assignment = "latest"
/// base-url = "../packages/"
///
/// [channel-relations]
/// base = "../conda-forge"
/// overrides = "../fallback"
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ChannelOptions {
    /// Whether to write `repodata.json.zst`.
    #[serde(default, alias = "write_zst")]
    pub write_zst: Option<bool>,
    /// Whether to write `repodata_shards.msgpack.zst` and shard files.
    #[serde(default, alias = "write_shards")]
    pub write_shards: Option<bool>,
    /// Repodata revisions to advertise in generated repodata.
    #[serde(
        default,
        alias = "repodata_revisions",
        deserialize_with = "deserialize_repodata_revisions"
    )]
    pub repodata_revisions: Vec<RepodataRevisionInfo>,
    /// How packages are assigned to repodata revisions.
    #[serde(default, alias = "package_revision_assignment")]
    pub package_revision_assignment: Option<PackageRevisionAssignment>,
    /// The `info.base_url` value written to generated repodata.
    #[serde(default, alias = "base_url")]
    pub base_url: Option<String>,
    /// Channel base/override relationships written to generated repodata.
    #[serde(default, alias = "channel_relations")]
    pub channel_relations: Option<ChannelRelations>,
}

impl ChannelOptions {
    /// Parse channel options from a TOML string.
    pub fn from_toml_str(contents: &str) -> anyhow::Result<Self> {
        let options: Self = toml::from_str(contents).context("failed to parse channel options")?;
        options.validate()?;
        Ok(options)
    }

    /// Load channel options from a TOML file.
    pub fn from_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let contents = fs_err::read_to_string(path)
            .with_context(|| format!("failed to read channel options from {}", path.display()))?;
        Self::from_toml_str(&contents)
            .with_context(|| format!("failed to load channel options from {}", path.display()))
    }

    /// Returns only the channel metadata fields from these options.
    pub fn channel_metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            base_url: self.base_url.clone(),
            channel_relations: self
                .channel_relations
                .clone()
                .filter(|relations| !relations.is_empty()),
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        if let Some(relations) = &self.channel_relations {
            if relations.base.is_some()
                && relations.overrides.is_some()
                && relations.base == relations.overrides
            {
                anyhow::bail!(
                    "`channel-relations.base` and `channel-relations.overrides` must not be the same channel"
                );
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct RepodataRevisionValue(RepodataRevision);

impl<'de> Deserialize<'de> for RepodataRevisionValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(RepodataRevisionVisitor)
    }
}

struct RepodataRevisionVisitor;

impl<'de> Visitor<'de> for RepodataRevisionVisitor {
    type Value = RepodataRevisionValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a repodata revision such as \"v3\", \"legacy\", or 3")
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: DeError,
    {
        let value = u64::try_from(value)
            .map_err(|_| E::custom("repodata revisions must not be negative"))?;
        Ok(RepodataRevisionValue(RepodataRevision::from(value)))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: DeError,
    {
        Ok(RepodataRevisionValue(RepodataRevision::from(value)))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: DeError,
    {
        RepodataRevision::from_str(value)
            .map(RepodataRevisionValue)
            .map_err(E::custom)
    }
}

fn deserialize_repodata_revisions<'de, D>(
    deserializer: D,
) -> Result<Vec<RepodataRevisionInfo>, D::Error>
where
    D: Deserializer<'de>,
{
    let revisions = Vec::<RepodataRevisionValue>::deserialize(deserializer)?;
    Ok(revisions
        .into_iter()
        .map(|revision| RepodataRevisionInfo {
            revision: revision.0,
            n_packages: None,
            oldest: None,
            newest: None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_channel_options() {
        let options = ChannelOptions::from_toml_str(
            r#"
write-shards = false
write-zst = false
repodata-revisions = ["v3"]
package-revision-assignment = "latest"
base-url = "../packages/"

[channel-relations]
base = "../conda-forge"
overrides = "../fallback"
"#,
        )
        .unwrap();

        assert_eq!(options.write_shards, Some(false));
        assert_eq!(options.write_zst, Some(false));
        assert_eq!(options.repodata_revisions.len(), 1);
        assert_eq!(options.repodata_revisions[0].revision, RepodataRevision::V3);
        assert_eq!(
            options.package_revision_assignment,
            Some(PackageRevisionAssignment::Latest)
        );
        assert_eq!(options.base_url.as_deref(), Some("../packages/"));
        assert_eq!(
            options.channel_relations.as_ref().unwrap().base.as_deref(),
            Some("../conda-forge")
        );
        assert_eq!(
            options
                .channel_relations
                .as_ref()
                .unwrap()
                .overrides
                .as_deref(),
            Some("../fallback")
        );
    }

    #[test]
    fn parses_numeric_repodata_revisions() {
        let options = ChannelOptions::from_toml_str("repodata-revisions = [3]\n").unwrap();
        assert_eq!(options.repodata_revisions[0].revision, RepodataRevision::V3);
    }

    #[test]
    fn rejects_matching_base_and_overrides() {
        let err = ChannelOptions::from_toml_str(
            r#"
[channel-relations]
base = "../same"
overrides = "../same"
"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("must not be the same channel"));
    }
}
