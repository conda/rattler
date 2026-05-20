use serde::{Deserialize, Serialize};

use crate::config::{Config, MergeError, ValidationError};
#[cfg(feature = "edit")]
use crate::edit::ConfigEditError;

/// Knobs for the link strategy used during package installation.
///
/// All fields are `Option<bool>` so that "unset" is distinguishable
/// from an explicit `true`/`false`. When unset, the package installer
/// uses its own default (typically: try every strategy, fall back as
/// needed).
///
/// Currently exposed as a standalone module — **not** wired into
/// `ConfigBase`. The intended embedding (`#[serde(flatten)]` so the
/// keys live at the top level of the TOML) interacts poorly with
/// `serde_ignored`: unknown top-level keys are no longer reported
/// when any sibling field uses `flatten`. Downstream consumers that
/// want typo detection on these keys should either embed under a
/// dedicated table (`[link-config]`) or keep equivalent flat fields
/// locally for now.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct LinkConfig {
    /// If set to false, symbolic links will not be used during package
    /// installation.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_symbolic_links: Option<bool>,

    /// If set to false, hard links will not be used during package
    /// installation.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_hard_links: Option<bool>,

    /// If set to false, ref links (copy-on-write) will not be used
    /// during package installation.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_ref_links: Option<bool>,
}

impl LinkConfig {
    /// Returns `true` when no link-strategy override is set. Useful for
    /// `#[serde(skip_serializing_if = "...")]` on the field.
    pub fn is_empty(&self) -> bool {
        self.allow_symbolic_links.is_none()
            && self.allow_hard_links.is_none()
            && self.allow_ref_links.is_none()
    }
}

impl Config for LinkConfig {
    fn get_extension_name(&self) -> String {
        "link".to_string()
    }

    fn merge_config(self, other: &Self) -> Result<Self, MergeError> {
        Ok(Self {
            allow_symbolic_links: other.allow_symbolic_links.or(self.allow_symbolic_links),
            allow_hard_links: other.allow_hard_links.or(self.allow_hard_links),
            allow_ref_links: other.allow_ref_links.or(self.allow_ref_links),
        })
    }

    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        vec![
            "allow-symbolic-links".to_string(),
            "allow-hard-links".to_string(),
            "allow-ref-links".to_string(),
        ]
    }

    #[cfg(feature = "edit")]
    fn set(&mut self, key: &str, value: Option<String>) -> Result<(), ConfigEditError> {
        let parse = |v: String| {
            v.parse::<bool>()
                .map_err(|e| ConfigEditError::BoolParseError {
                    key: key.to_string(),
                    source: e,
                })
        };
        match key {
            "allow-symbolic-links" => {
                self.allow_symbolic_links = value.map(parse).transpose()?;
            }
            "allow-hard-links" => {
                self.allow_hard_links = value.map(parse).transpose()?;
            }
            "allow-ref-links" => {
                self.allow_ref_links = value.map(parse).transpose()?;
            }
            _ => {
                return Err(ConfigEditError::UnknownKeyInner {
                    key: key.to_string(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty() {
        assert!(LinkConfig::default().is_empty());
    }

    #[test]
    fn merge_other_wins() {
        let base = LinkConfig {
            allow_symbolic_links: Some(true),
            allow_hard_links: None,
            allow_ref_links: Some(false),
        };
        let override_ = LinkConfig {
            allow_symbolic_links: Some(false),
            allow_hard_links: Some(true),
            allow_ref_links: None,
        };
        let merged = base.merge_config(&override_).unwrap();
        assert_eq!(merged.allow_symbolic_links, Some(false));
        assert_eq!(merged.allow_hard_links, Some(true));
        assert_eq!(merged.allow_ref_links, Some(false));
    }

    #[test]
    fn deserialize_kebab_case() {
        let toml = r#"
            allow-symbolic-links = true
            allow-hard-links = false
            allow-ref-links = true
        "#;
        let config: LinkConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.allow_symbolic_links, Some(true));
        assert_eq!(config.allow_hard_links, Some(false));
        assert_eq!(config.allow_ref_links, Some(true));
    }
}
