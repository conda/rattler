use std::fmt;

use indexmap::IndexMap;
use rattler_conda_types::PackageName;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor, value::MapAccessDeserializer},
};

/// The `exclude-newer` configuration that rattler-based tools start from when
/// nothing more specific is configured, e.g. the cutoff of `pixi exec` or of a
/// new workspace.
///
/// In the configuration this is either a bare cutoff:
///
/// ```toml
/// default-exclude-newer = "7d"
/// ```
///
/// or a table with a cutoff and exemptions:
///
/// ```toml
/// [default-exclude-newer]
/// cutoff = "7d"
/// exemptions = { ruff = "*" }
/// ```
///
/// The cutoff and the exemptions are kept as written, so that a tool can write
/// them into a manifest unchanged (e.g. `7d` instead of the point in time it
/// resolves to). Each tool parses them with its own `exclude-newer` parser:
/// the cutoff is usually an RFC 3339 timestamp, a date or a duration, and an
/// exemption is a version or a table that selects the exempt releases.
#[derive(Debug, Clone, PartialEq)]
pub struct ExcludeNewerConfig {
    /// Packages uploaded after this cutoff are excluded, e.g. `7d`.
    pub cutoff: String,

    /// The releases of a package that are not excluded, regardless of their
    /// timestamp, e.g. `ruff = "*"`.
    pub exemptions: IndexMap<PackageName, toml::Value>,
}

impl ExcludeNewerConfig {
    /// Creates a configuration that consists of a cutoff only, e.g. `7d`.
    pub fn from_cutoff(cutoff: impl Into<String>) -> Self {
        Self {
            cutoff: cutoff.into(),
            exemptions: IndexMap::new(),
        }
    }
}

/// The table form of [`ExcludeNewerConfig`].
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct ExcludeNewerTable {
    cutoff: String,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    exemptions: IndexMap<PackageName, toml::Value>,
}

impl Serialize for ExcludeNewerConfig {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Without exemptions, the cutoff is written on its own.
        if self.exemptions.is_empty() {
            return self.cutoff.serialize(serializer);
        }
        ExcludeNewerTable {
            cutoff: self.cutoff.clone(),
            exemptions: self.exemptions.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ExcludeNewerConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ExcludeNewerVisitor;

        impl<'de> Visitor<'de> for ExcludeNewerVisitor {
            type Value = ExcludeNewerConfig;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a cutoff string or a table with `cutoff` and `exemptions`")
            }

            fn visit_str<E: de::Error>(self, cutoff: &str) -> Result<Self::Value, E> {
                Ok(ExcludeNewerConfig::from_cutoff(cutoff))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                let table = ExcludeNewerTable::deserialize(MapAccessDeserializer::new(map))?;
                Ok(ExcludeNewerConfig {
                    cutoff: table.cutoff,
                    exemptions: table.exemptions,
                })
            }
        }

        deserializer.deserialize_any(ExcludeNewerVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct Document {
        default_exclude_newer: ExcludeNewerConfig,
    }

    #[test]
    fn test_parse_cutoff() {
        let document: Document = toml::from_str(r#"default-exclude-newer = "7d""#).unwrap();
        assert_eq!(
            document.default_exclude_newer,
            ExcludeNewerConfig::from_cutoff("7d")
        );
    }

    #[test]
    fn test_parse_table() {
        let document: Document = toml::from_str(
            r#"
            [default-exclude-newer]
            cutoff = "7d"
            exemptions = { ruff = "*", polars = { version = "==1.43.1", cutoff = "2026-10-12" } }
            "#,
        )
        .unwrap();
        let config = document.default_exclude_newer;
        assert_eq!(config.cutoff, "7d");
        assert_eq!(config.exemptions.len(), 2);
        assert_eq!(
            config.exemptions[&PackageName::new_unchecked("ruff")],
            toml::Value::String("*".to_string())
        );
        assert!(config.exemptions[&PackageName::new_unchecked("polars")].is_table());
    }

    #[test]
    fn test_parse_table_requires_cutoff() {
        let error = toml::from_str::<Document>(
            r#"
            [default-exclude-newer]
            exemptions = { ruff = "*" }
            "#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("missing field `cutoff`"));
    }

    #[test]
    fn test_parse_invalid_type() {
        let error = toml::from_str::<Document>("default-exclude-newer = 7").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("a cutoff string or a table with `cutoff` and `exemptions`")
        );
    }

    #[test]
    fn test_serialize_roundtrip() {
        for input in [
            "default-exclude-newer = \"7d\"\n",
            "[default-exclude-newer]\ncutoff = \"7d\"\n\n[default-exclude-newer.exemptions]\nruff = \"*\"\n",
        ] {
            let document: Document = toml::from_str(input).unwrap();
            assert_eq!(toml::to_string(&document).unwrap(), input);
        }
    }
}
