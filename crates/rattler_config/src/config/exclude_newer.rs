use std::{fmt, str::FromStr};

use indexmap::IndexMap;
use jiff::{Timestamp, civil::Date};
use rattler_conda_types::{
    BuildNumberSpec, PackageName, ParseStrictness, VersionSpec, match_spec::matcher::StringMatcher,
};
use rattler_digest::{Md5, Sha256, parse_digest_from_hex};
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
/// resolves to). They are validated when they are deserialized, see
/// [`Self::validate`]: the cutoff is an RFC 3339 timestamp, a date or a
/// duration, and an exemption is a version spec or a table of match spec
/// fields that select the exempt releases, with an optional absolute
/// `cutoff`.
///
/// Dates must be quoted. An unquoted TOML date, e.g.
/// `exclude-newer = 2026-10-12`, is rejected with a misleading error such as
/// ``missing field `cutoff` ``, because the `toml` crate passes dates to a
/// visitor as a map.
///
/// Keys inside the table can be edited on their own, e.g.
/// `exclude-newer.exemptions.ruff`, also when the cutoff is written on its
/// own.
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

/// An invalid cutoff or exemption of an [`ExcludeNewerConfig`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InvalidExcludeNewerError(String);

/// The match spec fields that select the exempt releases of a package and
/// take a single value.
const SINGLE_VALUE_FIELDS: &[&str] = &[
    "version",
    "build",
    "build-number",
    "file-name",
    "channel",
    "subdir",
    "license",
    "license-family",
    "md5",
    "sha256",
];

/// The match spec fields that select the exempt releases of a package and
/// take a list of values.
const LIST_FIELDS: &[&str] = &["flags", "track-features"];

impl ExcludeNewerConfig {
    /// Creates a configuration that consists of a cutoff only, e.g. `7d`.
    pub fn from_cutoff(cutoff: impl Into<String>) -> Self {
        Self {
            cutoff: cutoff.into(),
            exemptions: IndexMap::new(),
        }
    }

    /// Checks that the cutoff and the exemptions parse. This is done when the
    /// configuration is deserialized, so it is only needed for a
    /// configuration that is constructed directly.
    pub fn validate(&self) -> Result<(), InvalidExcludeNewerError> {
        validate_cutoff(&self.cutoff).map_err(InvalidExcludeNewerError)?;
        for (name, exemption) in &self.exemptions {
            exemption
                .validate()
                .map_err(|InvalidExcludeNewerError(err)| {
                    InvalidExcludeNewerError(format!(
                        "invalid exemption for `{}`: {err}",
                        name.as_source()
                    ))
                })?;
        }
        Ok(())
    }
}

impl ExcludeNewerExemption {
    /// Checks that the version spec or the match spec fields parse, and that
    /// a table selects the exempt releases by at least one match spec field.
    pub fn validate(&self) -> Result<(), InvalidExcludeNewerError> {
        match self {
            Self::Version(version) => validate_version(version),
            Self::Fields(fields) => validate_exemption_fields(fields),
        }
        .map_err(InvalidExcludeNewerError)
    }
}

/// Checks that `cutoff` is an RFC 3339 timestamp, a date or a duration.
fn validate_cutoff(cutoff: &str) -> Result<(), String> {
    if humantime::parse_duration(cutoff).is_ok() || validate_absolute_cutoff(cutoff).is_ok() {
        return Ok(());
    }
    Err(format!(
        "invalid cutoff `{cutoff}`, expected an RFC 3339 timestamp, a date, or a duration (for example, 7d)"
    ))
}

/// Checks that `cutoff` is an RFC 3339 timestamp or a date, the cutoffs that
/// don't move over time.
fn validate_absolute_cutoff(cutoff: &str) -> Result<(), String> {
    if cutoff.parse::<Timestamp>().is_ok() || cutoff.parse::<Date>().is_ok() {
        return Ok(());
    }
    Err(format!(
        "invalid cutoff `{cutoff}`, expected an RFC 3339 timestamp or a date"
    ))
}

/// Checks that `version` is a version spec. Lenient, so that the tools can
/// apply their own strictness.
fn validate_version(version: &str) -> Result<(), String> {
    VersionSpec::from_str(version, ParseStrictness::Lenient)
        .map(drop)
        .map_err(|err| format!("invalid version `{version}`: {err}"))
}

/// Checks the match spec fields of an exemption table. An empty table would
/// exempt every release, so a typo would silently exempt the package
/// entirely.
fn validate_exemption_fields(
    fields: &IndexMap<String, ExcludeNewerExemptionField>,
) -> Result<(), String> {
    for (name, value) in fields {
        validate_exemption_field(name, value)?;
    }
    if fields.keys().all(|name| name == "cutoff") {
        return Err("an exemption table must specify at least one match spec field besides `cutoff`, use `version = \"*\"` to exempt every release".to_string());
    }
    Ok(())
}

/// Checks a match spec field of an exemption table.
fn validate_exemption_field(name: &str, value: &ExcludeNewerExemptionField) -> Result<(), String> {
    use ExcludeNewerExemptionField::{List, Single};

    let parse_string_matcher = |value: &str| {
        StringMatcher::from_str(value)
            .map(drop)
            .map_err(|err| format!("invalid `{name}` `{value}`: {err}"))
    };
    match (name, value) {
        ("cutoff", Single(cutoff)) => validate_absolute_cutoff(cutoff),
        ("version", Single(version)) => validate_version(version),
        ("build", Single(build)) => parse_string_matcher(build),
        ("build-number", Single(build_number)) => BuildNumberSpec::from_str(build_number)
            .map(drop)
            .map_err(|err| format!("invalid `build-number` `{build_number}`: {err}")),
        ("md5", Single(md5)) => parse_digest_from_hex::<Md5>(md5)
            .map(drop)
            .ok_or_else(|| format!("invalid `md5` `{md5}`, expected a hex encoded MD5 hash")),
        ("sha256", Single(sha256)) => parse_digest_from_hex::<Sha256>(sha256)
            .map(drop)
            .ok_or_else(|| {
                format!("invalid `sha256` `{sha256}`, expected a hex encoded SHA256 hash")
            }),
        ("flags", List(flags)) => flags.iter().try_for_each(|flag| parse_string_matcher(flag)),
        (name, Single(_)) if SINGLE_VALUE_FIELDS.contains(&name) => Ok(()),
        (name, List(_)) if LIST_FIELDS.contains(&name) => Ok(()),
        ("cutoff", List(_)) => Err("`cutoff` must be a string".to_string()),
        (name, List(_)) if SINGLE_VALUE_FIELDS.contains(&name) => {
            Err(format!("`{name}` must be a string"))
        }
        (name, Single(_)) if LIST_FIELDS.contains(&name) => {
            Err(format!("`{name}` must be an array of strings"))
        }
        (name, _) => Err(format!(
            "unknown field `{name}`, expected `cutoff` or one of the match spec fields {}",
            SINGLE_VALUE_FIELDS
                .iter()
                .chain(LIST_FIELDS)
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
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
                let exemption: ExcludeNewerExemption = map.next_value()?;
                exemption.validate().map_err(|err| {
                    de::Error::custom(format!(
                        "invalid exemption for `{}`: {err}",
                        name.as_source()
                    ))
                })?;
                exemptions.insert(name, exemption);
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
                validate_cutoff(cutoff).map_err(E::custom)?;
                Ok(ExcludeNewerConfig::from_cutoff(cutoff))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                let table = ExcludeNewerTable::deserialize(MapAccessDeserializer::new(map))?;
                // The exemptions are validated while they are deserialized.
                validate_cutoff(&table.cutoff).map_err(de::Error::custom)?;
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
    fn test_parse_valid_cutoffs() {
        for cutoff in [
            "7d",
            "1week 2days",
            "12h",
            "2026-10-12",
            "2026-10-12T05:00:00Z",
            "2026-10-12T05:00:00+02:00",
        ] {
            let input = format!(r#"exclude-newer = "{cutoff}""#);
            let document: Document = toml::from_str(&input).unwrap();
            assert_eq!(document.exclude_newer.cutoff, cutoff);
        }
    }

    #[test]
    fn test_parse_invalid_cutoff() {
        for input in [
            r#"exclude-newer = "7 weeks ago""#,
            r#"exclude-newer = { cutoff = "yesterday" }"#,
            r#"exclude-newer = "2026-13-01""#,
        ] {
            let error = toml::from_str::<Document>(input).unwrap_err();
            assert!(
                error.to_string().contains(
                    "expected an RFC 3339 timestamp, a date, or a duration (for example, 7d)"
                ),
                "{input}: {error}"
            );
        }
    }

    #[test]
    fn test_parse_valid_exemptions() {
        let document: Document = toml::from_str(
            r#"
            [exclude-newer]
            cutoff = "7d"
            [exclude-newer.exemptions]
            ruff = ">=0.1,<1"
            polars = "1.43.1"
            numpy = { version = "*", build = "py3*", build-number = ">=2", cutoff = "2026-10-12" }
            pytorch = { flags = ["cuda*"], track-features = ["mkl"], channel = "conda-forge", subdir = "linux-64" }
            openssl = { sha256 = "0000000000000000000000000000000000000000000000000000000000000000", md5 = "00000000000000000000000000000000" }
            libfoo = { file-name = "libfoo-1-0.conda", license = "MIT", license-family = "MIT" }
            "#,
        )
        .unwrap();
        assert_eq!(document.exclude_newer.exemptions.len(), 6);
    }

    #[test]
    fn test_parse_invalid_exemptions() {
        for (exemption, message) in [
            (r#"">>>""#, "invalid version `>>>`"),
            (r#"{ version = ">>>" }"#, "invalid version `>>>`"),
            (r#"{ build-number = "x" }"#, "invalid `build-number` `x`"),
            (r#"{ md5 = "abc" }"#, "invalid `md5` `abc`"),
            (r#"{ sha256 = "abc" }"#, "invalid `sha256` `abc`"),
            (
                r#"{ flags = "cuda" }"#,
                "`flags` must be an array of strings",
            ),
            (r#"{ build = ["py3"] }"#, "`build` must be a string"),
            (
                r#"{ version = "*", cutoff = "7d" }"#,
                "invalid cutoff `7d`, expected an RFC 3339 timestamp or a date",
            ),
            (r#"{ bogus = "*" }"#, "unknown field `bogus`"),
            (r#"{ extras = ["x"] }"#, "unknown field `extras`"),
            (
                "{}",
                "an exemption table must specify at least one match spec field besides `cutoff`",
            ),
            (
                r#"{ cutoff = "2026-10-12" }"#,
                "an exemption table must specify at least one match spec field besides `cutoff`",
            ),
        ] {
            let input = format!(
                r#"exclude-newer = {{ cutoff = "7d", exemptions = {{ ruff = {exemption} }} }}"#
            );
            let error = toml::from_str::<Document>(&input).unwrap_err();
            assert!(
                error.to_string().contains("invalid exemption for `ruff`")
                    && error.to_string().contains(message),
                "{exemption}: {error}"
            );
        }
    }

    #[test]
    fn test_validate() {
        assert!(ExcludeNewerConfig::from_cutoff("7d").validate().is_ok());
        assert_eq!(
            ExcludeNewerConfig::from_cutoff("7 weeks ago")
                .validate()
                .unwrap_err()
                .to_string(),
            "invalid cutoff `7 weeks ago`, expected an RFC 3339 timestamp, a date, or a duration (for example, 7d)"
        );
        let mut config = ExcludeNewerConfig::from_cutoff("7d");
        config.exemptions.insert(
            PackageName::new_unchecked("ruff"),
            ExcludeNewerExemption::Version(">>>".to_string()),
        );
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .starts_with("invalid exemption for `ruff`: invalid version `>>>`")
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
