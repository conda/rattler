use std::fmt;

use indexmap::IndexMap;
use rattler_conda_types::PackageName;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, SeqAccess, Visitor, value::MapAccessDeserializer},
};

/// The `exclude-newer` configuration that rattler-based tools start from when
/// nothing more specific is configured, e.g. the cutoff of `pixi exec` or of a
/// new workspace.
///
/// In the configuration this is either a bare cutoff:
///
/// ```toml
/// exclude-newer = "7d"
/// ```
///
/// or a table with a cutoff and exemptions:
///
/// ```toml
/// [exclude-newer]
/// cutoff = "7d"
/// exemptions = { ruff = "*" }
/// ```
///
/// The cutoff and the exemptions are kept as written, so that a tool can write
/// them into a manifest unchanged (e.g. `7d` instead of the point in time it
/// resolves to). Each tool parses them with its own `exclude-newer` parser:
/// the cutoff is usually an RFC 3339 timestamp, a date or a duration, and an
/// exemption is a version or a table that selects the exempt releases.
///
/// Dates must be quoted. An unquoted TOML date, e.g.
/// `exclude-newer = 2026-10-12`, is rejected with a misleading error such as
/// ``missing field `cutoff` ``, because the `toml` crate passes dates to a
/// visitor as a map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludeNewerConfig {
    /// Packages uploaded after this cutoff are excluded, e.g. `7d`.
    pub cutoff: String,

    /// The releases of a package that are not excluded, regardless of their
    /// timestamp, e.g. `ruff = "*"`.
    pub exemptions: IndexMap<PackageName, ExcludeNewerExemption>,
}

/// The releases of a package that are exempt from the `exclude-newer` cutoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ExcludeNewerExemption {
    /// A version spec, e.g. `"*"` or `"==1.43.1"`.
    Version(String),

    /// Match spec fields that select the exempt releases, e.g.
    /// `{ version = "==1.43.1", cutoff = "2026-10-12" }`.
    Fields(IndexMap<String, ExcludeNewerExemptionField>),
}

/// The value of a match spec field of an [`ExcludeNewerExemption`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ExcludeNewerExemptionField {
    /// A single value, e.g. `version = "==1.43.1"`.
    Single(String),

    /// A list of values, e.g. `flags = ["cuda"]`.
    List(Vec<String>),
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
    #[serde(
        default,
        skip_serializing_if = "IndexMap::is_empty",
        deserialize_with = "deserialize_exemptions"
    )]
    exemptions: IndexMap<PackageName, ExcludeNewerExemption>,
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

impl<'de> Deserialize<'de> for ExcludeNewerExemptionField {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldVisitor;

        impl<'de> Visitor<'de> for FieldVisitor {
            type Value = ExcludeNewerExemptionField;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string or an array of strings")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(ExcludeNewerExemptionField::Single(value.to_string()))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element()? {
                    values.push(value);
                }
                Ok(ExcludeNewerExemptionField::List(values))
            }
        }

        deserializer.deserialize_any(FieldVisitor)
    }
}

impl<'de> Deserialize<'de> for ExcludeNewerExemption {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ExemptionVisitor;

        impl<'de> Visitor<'de> for ExemptionVisitor {
            type Value = ExcludeNewerExemption;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a version string or a table of match spec fields")
            }

            fn visit_str<E: de::Error>(self, version: &str) -> Result<Self::Value, E> {
                Ok(ExcludeNewerExemption::Version(version.to_string()))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut fields = IndexMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    fields.insert(key, map.next_value()?);
                }
                Ok(ExcludeNewerExemption::Fields(fields))
            }
        }

        deserializer.deserialize_any(ExemptionVisitor)
    }
}

/// Deserializes the exemptions of an [`ExcludeNewerConfig`]. Package names are
/// compared normalized, so names that differ only in case are rejected as
/// duplicates instead of silently overwriting each other.
fn deserialize_exemptions<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<IndexMap<PackageName, ExcludeNewerExemption>, D::Error> {
    struct ExemptionsVisitor;

    impl<'de> Visitor<'de> for ExemptionsVisitor {
        type Value = IndexMap<PackageName, ExcludeNewerExemption>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a table of exemptions")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut exemptions = Self::Value::new();
            while let Some(name) = map.next_key::<PackageName>()? {
                if let Some((existing, _)) = exemptions.get_key_value(&name) {
                    return Err(de::Error::custom(format!(
                        "duplicate exemption for `{}`, already exempted as `{}`",
                        name.as_source(),
                        existing.as_source(),
                    )));
                }
                exemptions.insert(name, map.next_value()?);
            }
            Ok(exemptions)
        }
    }

    deserializer.deserialize_map(ExemptionsVisitor)
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
        exclude_newer: ExcludeNewerConfig,
    }

    #[test]
    fn test_parse_cutoff() {
        let document: Document = toml::from_str(r#"exclude-newer = "7d""#).unwrap();
        assert_eq!(
            document.exclude_newer,
            ExcludeNewerConfig::from_cutoff("7d")
        );
    }

    #[test]
    fn test_parse_table() {
        let document: Document = toml::from_str(
            r#"
            [exclude-newer]
            cutoff = "7d"
            exemptions = { ruff = "*", polars = { version = "==1.43.1", cutoff = "2026-10-12" } }
            "#,
        )
        .unwrap();
        let config = document.exclude_newer;
        assert_eq!(config.cutoff, "7d");
        assert_eq!(config.exemptions.len(), 2);
        assert_eq!(
            config.exemptions[&PackageName::new_unchecked("ruff")],
            ExcludeNewerExemption::Version("*".to_string())
        );
        assert_eq!(
            config.exemptions[&PackageName::new_unchecked("polars")],
            ExcludeNewerExemption::Fields(IndexMap::from([
                (
                    "version".to_string(),
                    ExcludeNewerExemptionField::Single("==1.43.1".to_string())
                ),
                (
                    "cutoff".to_string(),
                    ExcludeNewerExemptionField::Single("2026-10-12".to_string())
                ),
            ]))
        );
    }

    #[test]
    fn test_parse_exemption_list_field() {
        let document: Document = toml::from_str(
            r#"exclude-newer = { cutoff = "7d", exemptions = { pytorch = { flags = ["cuda"] } } }"#,
        )
        .unwrap();
        assert_eq!(
            document.exclude_newer.exemptions[&PackageName::new_unchecked("pytorch")],
            ExcludeNewerExemption::Fields(IndexMap::from([(
                "flags".to_string(),
                ExcludeNewerExemptionField::List(vec!["cuda".to_string()])
            )]))
        );
    }

    #[test]
    fn test_parse_exemption_rejects_non_strings() {
        for exemption in ["1.1", "nan", "2", "true", "{ version = 1 }"] {
            let input = format!(
                r#"exclude-newer = {{ cutoff = "7d", exemptions = {{ ruff = {exemption} }} }}"#
            );
            assert!(
                toml::from_str::<Document>(&input).is_err(),
                "accepted {exemption}"
            );
        }
    }

    #[test]
    fn test_parse_duplicate_exemption() {
        let error = toml::from_str::<Document>(
            r#"exclude-newer = { cutoff = "7d", exemptions = { Ruff = "*", ruff = "==1" } }"#,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("duplicate exemption for `ruff`, already exempted as `Ruff`"),
            "{error}"
        );
    }

    #[test]
    fn test_parse_table_requires_cutoff() {
        let error = toml::from_str::<Document>(
            r#"
            [exclude-newer]
            exemptions = { ruff = "*" }
            "#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("missing field `cutoff`"));
    }

    #[test]
    fn test_parse_invalid_type() {
        let error = toml::from_str::<Document>("exclude-newer = 7").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("a cutoff string or a table with `cutoff` and `exemptions`")
        );
    }

    #[test]
    fn test_serialize_roundtrip() {
        for input in [
            "exclude-newer = \"7d\"\n",
            "[exclude-newer]\ncutoff = \"7d\"\n\n[exclude-newer.exemptions]\nruff = \"*\"\n",
        ] {
            let document: Document = toml::from_str(input).unwrap();
            assert_eq!(toml::to_string(&document).unwrap(), input);
        }
    }
}
