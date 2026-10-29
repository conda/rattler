use std::{fmt, str::FromStr, sync::Arc, time::Duration};

use indexmap::IndexMap;
use jiff::{Span, Timestamp, civil::Date, tz::TimeZone};
use rattler_conda_types::{
    BuildNumberSpec, Channel, ChannelConfig, MatchSpec, NamelessMatchSpec, PackageName,
    PackageNameMatcher, ParseStrictness, VersionSpec, match_spec::matcher::StringMatcher,
};
use rattler_digest::{Md5, Sha256, parse_digest_from_hex};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor, value::MapAccessDeserializer},
};
use url::Url;

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
/// The cutoffs are kept as written, so that a tool can write them into a
/// manifest unchanged (e.g. `7d` instead of the point in time it resolves
/// to). They are validated when they are deserialized, see [`Self::validate`]:
/// the cutoff is an RFC 3339 timestamp, a date or a duration. An exemption is
/// a version spec or a table of match spec fields that select the exempt
/// releases, with an optional absolute `cutoff`. It is parsed into a
/// [`NamelessMatchSpec`].
///
/// Use [`Self::to_exclude_newer_with_now`] or `TryFrom` to convert the
/// configuration into a [`rattler_solve::ExcludeNewer`].
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
///
/// In the configuration this is either a version spec, e.g. `"==1.43.1"`, or
/// a table of match spec fields, e.g.
/// `{ version = "==1.43.1", cutoff = "2026-10-12" }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludeNewerExemption {
    /// The releases that are exempt. Only the fields that select records are
    /// supported, see [`rattler_solve::ExcludeNewer::with_exemption`].
    pub spec: NamelessMatchSpec,

    /// Only the releases published up to this cutoff are exempt, an RFC 3339
    /// timestamp or a date, e.g. `2026-10-12`.
    pub cutoff: Option<String>,
}

/// An invalid cutoff or exemption of an [`ExcludeNewerConfig`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InvalidExcludeNewerError(String);

/// A parsed cutoff of an [`ExcludeNewerConfig`].
enum Cutoff {
    Timestamp(Timestamp),
    Duration(Duration),
}

impl ExcludeNewerConfig {
    /// Creates a configuration that consists of a cutoff only, e.g. `7d`.
    pub fn from_cutoff(cutoff: impl Into<String>) -> Self {
        Self {
            cutoff: cutoff.into(),
            exemptions: IndexMap::new(),
        }
    }

    /// Checks that the cutoff parses and that the exemptions are valid. This
    /// is done when the configuration is deserialized, so it is only needed
    /// for a configuration that is constructed directly.
    pub fn validate(&self) -> Result<(), InvalidExcludeNewerError> {
        self.to_exclude_newer_with_now(Timestamp::now()).map(drop)
    }

    /// Converts the configuration into a [`rattler_solve::ExcludeNewer`],
    /// resolving a duration cutoff relative to `now`.
    pub fn to_exclude_newer_with_now(
        &self,
        now: Timestamp,
    ) -> Result<rattler_solve::ExcludeNewer, InvalidExcludeNewerError> {
        let cutoff = resolve_cutoff(&self.cutoff, now).map_err(InvalidExcludeNewerError)?;
        let mut exclude_newer = rattler_solve::ExcludeNewer::from_datetime(cutoff);
        for (name, exemption) in &self.exemptions {
            exclude_newer = exemption.add_to(exclude_newer, name).map_err(|err| {
                InvalidExcludeNewerError(format!(
                    "invalid exemption for `{}`: {err}",
                    name.as_source()
                ))
            })?;
        }
        Ok(exclude_newer)
    }
}

impl TryFrom<&ExcludeNewerConfig> for rattler_solve::ExcludeNewer {
    type Error = InvalidExcludeNewerError;

    /// Converts the configuration, resolving a duration cutoff relative to
    /// the current time.
    fn try_from(config: &ExcludeNewerConfig) -> Result<Self, Self::Error> {
        config.to_exclude_newer_with_now(Timestamp::now())
    }
}

impl TryFrom<ExcludeNewerConfig> for rattler_solve::ExcludeNewer {
    type Error = InvalidExcludeNewerError;

    /// Converts the configuration, resolving a duration cutoff relative to
    /// the current time.
    fn try_from(config: ExcludeNewerConfig) -> Result<Self, Self::Error> {
        Self::try_from(&config)
    }
}

impl ExcludeNewerExemption {
    /// Creates an exemption for the releases matching a version spec.
    pub fn from_version(version: VersionSpec) -> Self {
        Self {
            spec: NamelessMatchSpec {
                version: Some(version),
                ..NamelessMatchSpec::default()
            },
            cutoff: None,
        }
    }

    /// Adds this exemption of the package `name` to `exclude_newer`.
    fn add_to(
        &self,
        exclude_newer: rattler_solve::ExcludeNewer,
        name: &PackageName,
    ) -> Result<rattler_solve::ExcludeNewer, String> {
        let cutoff = self
            .cutoff
            .as_deref()
            .map(parse_absolute_cutoff)
            .transpose()?;
        let spec =
            MatchSpec::from_nameless(self.spec.clone(), PackageNameMatcher::Exact(name.clone()));
        match cutoff {
            Some(cutoff) => exclude_newer.with_exemption_until(spec, cutoff),
            None => exclude_newer.with_exemption(spec),
        }
        .map_err(|err| err.to_string())
    }

    /// Returns the version spec if the exemption consists of it only, which
    /// is written on its own.
    fn as_version(&self) -> Option<&VersionSpec> {
        let version = self.spec.version.as_ref()?;
        let only_version = NamelessMatchSpec {
            version: Some(version.clone()),
            ..NamelessMatchSpec::default()
        };
        (self.cutoff.is_none() && self.spec == only_version).then_some(version)
    }
}

/// Parses `cutoff` and resolves a duration relative to `now`.
fn resolve_cutoff(cutoff: &str, now: Timestamp) -> Result<Timestamp, String> {
    match parse_cutoff(cutoff)? {
        Cutoff::Timestamp(timestamp) => Ok(timestamp),
        Cutoff::Duration(duration) => Span::try_from(duration)
            .ok()
            .and_then(|span| now.checked_sub(span).ok())
            .ok_or_else(|| format!("cutoff `{cutoff}` is too large")),
    }
}

/// Parses `cutoff` as an RFC 3339 timestamp, a date or a duration.
fn parse_cutoff(cutoff: &str) -> Result<Cutoff, String> {
    if let Ok(duration) = humantime::parse_duration(cutoff) {
        return Ok(Cutoff::Duration(duration));
    }
    parse_absolute_cutoff(cutoff).map(Cutoff::Timestamp).map_err(|_err| {
        format!(
            "invalid cutoff `{cutoff}`, expected an RFC 3339 timestamp, a date, or a duration (for example, 7d)"
        )
    })
}

/// Parses `cutoff` as an RFC 3339 timestamp or a date, the cutoffs that don't
/// move over time. A date includes the entire day in UTC.
fn parse_absolute_cutoff(cutoff: &str) -> Result<Timestamp, String> {
    if let Ok(timestamp) = cutoff.parse::<Timestamp>() {
        return Ok(timestamp);
    }
    // Only a plain date, `Date::from_str` also accepts a date-time and drops
    // its time.
    Date::strptime("%Y-%m-%d", cutoff)
        .ok()
        .and_then(|date| date.tomorrow().ok())
        .and_then(|next_day| next_day.to_zoned(TimeZone::UTC).ok())
        .map(|next_day| next_day.timestamp())
        .ok_or_else(|| {
            format!("invalid cutoff `{cutoff}`, expected an RFC 3339 timestamp or a date")
        })
}

/// Parses `version` as a version spec. Lenient, so that the tools can apply
/// their own strictness.
fn parse_version(version: &str) -> Result<VersionSpec, String> {
    VersionSpec::from_str(version, ParseStrictness::Lenient)
        .map_err(|err| format!("invalid version `{version}`: {err}"))
}

/// The configuration that the `channel` of an exemption is resolved with.
fn channel_config() -> ChannelConfig {
    ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap_or_default())
}

/// The table form of an [`ExcludeNewerExemption`]. Only the match spec fields
/// that select records are supported.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct ExemptionTable {
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    build: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    build_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subdir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    license_family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    md5: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    flags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    track_features: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cutoff: Option<String>,
}

impl TryFrom<ExemptionTable> for ExcludeNewerExemption {
    type Error = String;

    fn try_from(table: ExemptionTable) -> Result<Self, Self::Error> {
        let parse_string_matcher = |field: &str, value: String| {
            StringMatcher::from_str(&value)
                .map_err(|err| format!("invalid `{field}` `{value}`: {err}"))
        };
        let channel_config = channel_config();
        let spec = NamelessMatchSpec {
            version: table.version.as_deref().map(parse_version).transpose()?,
            build: table
                .build
                .map(|build| parse_string_matcher("build", build))
                .transpose()?,
            build_number: table
                .build_number
                .map(|build_number| {
                    BuildNumberSpec::from_str(&build_number)
                        .map_err(|err| format!("invalid `build-number` `{build_number}`: {err}"))
                })
                .transpose()?,
            file_name: table.file_name,
            channel: table
                .channel
                .map(|channel| {
                    Channel::from_str(&channel, &channel_config)
                        .map(Arc::new)
                        .map_err(|err| format!("invalid `channel` `{channel}`: {err}"))
                })
                .transpose()?,
            subdir: table.subdir,
            license: table.license,
            license_family: table.license_family,
            md5: table
                .md5
                .map(|md5| {
                    parse_digest_from_hex::<Md5>(&md5).ok_or_else(|| {
                        format!("invalid `md5` `{md5}`, expected a hex encoded MD5 hash")
                    })
                })
                .transpose()?,
            sha256: table
                .sha256
                .map(|sha256| {
                    parse_digest_from_hex::<Sha256>(&sha256).ok_or_else(|| {
                        format!("invalid `sha256` `{sha256}`, expected a hex encoded SHA256 hash")
                    })
                })
                .transpose()?,
            url: table
                .url
                .map(|url| Url::parse(&url).map_err(|err| format!("invalid `url` `{url}`: {err}")))
                .transpose()?,
            flags: table
                .flags
                .map(|flags| {
                    flags
                        .into_iter()
                        .map(|flag| parse_string_matcher("flags", flag))
                        .collect()
                })
                .transpose()?,
            track_features: table.track_features,
            ..NamelessMatchSpec::default()
        };
        // An empty table would exempt every release, so a typo would silently
        // exempt the package entirely. An empty list matches every release
        // too, so it doesn't count.
        let selecting_spec = NamelessMatchSpec {
            flags: spec.flags.clone().filter(|flags| !flags.is_empty()),
            track_features: spec
                .track_features
                .clone()
                .filter(|track_features| !track_features.is_empty()),
            ..spec.clone()
        };
        if selecting_spec == NamelessMatchSpec::default() {
            return Err("an exemption table must specify at least one match spec field besides `cutoff` that is not an empty list, use `version = \"*\"` to exempt every release".to_string());
        }
        if let Some(cutoff) = &table.cutoff {
            parse_absolute_cutoff(cutoff)?;
        }
        Ok(Self {
            spec,
            cutoff: table.cutoff,
        })
    }
}

impl From<&ExcludeNewerExemption> for ExemptionTable {
    fn from(exemption: &ExcludeNewerExemption) -> Self {
        let spec = &exemption.spec;
        Self {
            version: spec.version.as_ref().map(ToString::to_string),
            build: spec.build.as_ref().map(ToString::to_string),
            build_number: spec.build_number.as_ref().map(ToString::to_string),
            file_name: spec.file_name.clone(),
            channel: spec.channel.as_ref().map(|channel| {
                // The name of a channel is written only if it resolves to the
                // same channel, e.g. not for `https://prefix.dev/conda-forge`.
                let name = channel.name();
                match Channel::from_str(name, &channel_config()) {
                    Ok(resolved) if resolved.base_url == channel.base_url => name.to_string(),
                    _ => channel.base_url.to_string(),
                }
            }),
            subdir: spec.subdir.clone(),
            license: spec.license.clone(),
            license_family: spec.license_family.clone(),
            md5: spec.md5.as_ref().map(hex::encode),
            sha256: spec.sha256.as_ref().map(hex::encode),
            url: spec.url.as_ref().map(ToString::to_string),
            flags: spec
                .flags
                .as_ref()
                .map(|flags| flags.iter().map(ToString::to_string).collect()),
            track_features: spec.track_features.clone(),
            cutoff: exemption.cutoff.clone(),
        }
    }
}

impl Serialize for ExcludeNewerExemption {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.as_version() {
            Some(version) => version.to_string().serialize(serializer),
            None => ExemptionTable::from(self).serialize(serializer),
        }
    }
}

/// An [`ExcludeNewerExemption`] as written, before it is parsed.
enum RawExemption {
    Version(String),
    Table(Box<ExemptionTable>),
}

impl TryFrom<RawExemption> for ExcludeNewerExemption {
    type Error = String;

    fn try_from(raw: RawExemption) -> Result<Self, Self::Error> {
        match raw {
            RawExemption::Version(version) => parse_version(&version).map(Self::from_version),
            RawExemption::Table(table) => (*table).try_into(),
        }
    }
}

impl<'de> Deserialize<'de> for RawExemption {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ExemptionVisitor;

        impl<'de> Visitor<'de> for ExemptionVisitor {
            type Value = RawExemption;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a version string or a table of match spec fields")
            }

            fn visit_str<E: de::Error>(self, version: &str) -> Result<Self::Value, E> {
                Ok(RawExemption::Version(version.to_string()))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                ExemptionTable::deserialize(MapAccessDeserializer::new(map))
                    .map(|table| RawExemption::Table(Box::new(table)))
            }
        }

        deserializer.deserialize_any(ExemptionVisitor)
    }
}

impl<'de> Deserialize<'de> for ExcludeNewerExemption {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        RawExemption::deserialize(deserializer)?
            .try_into()
            .map_err(de::Error::custom)
    }
}

/// The table form of [`ExcludeNewerConfig`].
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
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
                let invalid_exemption = |err: &dyn fmt::Display| {
                    de::Error::custom(format!(
                        "invalid exemption for `{}`: {err}",
                        name.as_source()
                    ))
                };
                let exemption = map
                    .next_value::<RawExemption>()
                    .map_err(|err| invalid_exemption(&err))?
                    .try_into()
                    .map_err(|err: String| invalid_exemption(&err))?;
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
                resolve_cutoff(cutoff, Timestamp::now()).map_err(E::custom)?;
                Ok(ExcludeNewerConfig::from_cutoff(cutoff))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                let table = ExcludeNewerTable::deserialize(MapAccessDeserializer::new(map))?;
                // The exemptions are validated while they are deserialized.
                resolve_cutoff(&table.cutoff, Timestamp::now()).map_err(de::Error::custom)?;
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
            ExcludeNewerExemption::from_version(version("*"))
        );
        assert_eq!(
            config.exemptions[&PackageName::new_unchecked("polars")],
            ExcludeNewerExemption {
                cutoff: Some("2026-10-12".to_string()),
                ..ExcludeNewerExemption::from_version(version("==1.43.1"))
            }
        );
    }

    fn version(version: &str) -> VersionSpec {
        VersionSpec::from_str(version, ParseStrictness::Lenient).unwrap()
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
            // A date-time needs an offset.
            r#"exclude-newer = "2026-10-12T05:00:00""#,
            r#"exclude-newer = "2026-10-12 05:00""#,
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
    fn test_parse_too_large_cutoff() {
        let error = toml::from_str::<Document>(r#"exclude-newer = "100000y""#).unwrap_err();
        assert!(
            error.to_string().contains("cutoff `100000y` is too large"),
            "{error}"
        );
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
            pytorch = { channel = "conda-forge", subdir = "linux-64" }
            openssl = { sha256 = "0000000000000000000000000000000000000000000000000000000000000000", md5 = "00000000000000000000000000000000" }
            libfoo = { file-name = "libfoo-1-0.conda", license = "MIT", license-family = "MIT" }
            libbar = { url = "https://example.com/libbar-1-0.conda" }
            torch = { flags = ["cuda*"], track-features = ["mkl"] }
            jax = { version = "*", flags = [], track-features = [] }
            "#,
        )
        .unwrap();
        assert_eq!(document.exclude_newer.exemptions.len(), 9);
        document.exclude_newer.validate().unwrap();
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
                "invalid type: string \"cuda\", expected a sequence",
            ),
            (
                r#"{ build = ["py3"] }"#,
                "invalid type: sequence, expected a string",
            ),
            (r#"{ url = "not a url" }"#, "invalid `url` `not a url`"),
            (
                r#"{ version = "*", cutoff = "7d" }"#,
                "invalid cutoff `7d`, expected an RFC 3339 timestamp or a date",
            ),
            (
                r#"{ version = "*", cutoff = "2026-10-12T05:00:00" }"#,
                "invalid cutoff `2026-10-12T05:00:00`, expected an RFC 3339 timestamp or a date",
            ),
            (r#"{ bogus = "*" }"#, "unknown field `bogus`"),
            (r#"{ extras = ["x"] }"#, "unknown field `extras`"),
            (r#"{ namespace = "x" }"#, "unknown field `namespace`"),
            (
                "{}",
                "an exemption table must specify at least one match spec field besides `cutoff`",
            ),
            (
                r#"{ cutoff = "2026-10-12" }"#,
                "an exemption table must specify at least one match spec field besides `cutoff`",
            ),
            (
                r#"{ flags = [], track-features = [] }"#,
                "an exemption table must specify at least one match spec field besides `cutoff` that is not an empty list",
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
        for (exemption, message) in [
            (
                ExcludeNewerExemption {
                    spec: NamelessMatchSpec {
                        extras: Some(vec!["x".to_string()]),
                        ..NamelessMatchSpec::default()
                    },
                    cutoff: None,
                },
                "invalid exemption for `ruff`: exclude-newer exemption 'ruff[extras=[x]]' must not have extras",
            ),
            (
                ExcludeNewerExemption {
                    cutoff: Some("7d".to_string()),
                    ..ExcludeNewerExemption::from_version(version("*"))
                },
                "invalid exemption for `ruff`: invalid cutoff `7d`, expected an RFC 3339 timestamp or a date",
            ),
        ] {
            let mut config = ExcludeNewerConfig::from_cutoff("7d");
            config
                .exemptions
                .insert(PackageName::new_unchecked("ruff"), exemption);
            let error = config.validate().unwrap_err().to_string();
            assert!(error.starts_with(message), "{error}");
        }
    }

    #[test]
    fn test_to_exclude_newer() {
        let document: Document = toml::from_str(
            r#"
            [exclude-newer]
            cutoff = "1d"
            exemptions = { ruff = ">=0.5,<0.6", polars = { version = "==1.43.1", cutoff = "2026-10-12" } }
            "#,
        )
        .unwrap();
        let now: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
        let exclude_newer = document
            .exclude_newer
            .to_exclude_newer_with_now(now)
            .unwrap();

        let expected =
            rattler_solve::ExcludeNewer::from_datetime("2026-10-19T00:00:00Z".parse().unwrap())
                .with_exemption(
                    MatchSpec::from_str("ruff >=0.5,<0.6", ParseStrictness::Lenient).unwrap(),
                )
                .unwrap()
                .with_exemption_until(
                    MatchSpec::from_str("polars ==1.43.1", ParseStrictness::Lenient).unwrap(),
                    // A date includes the entire day.
                    "2026-10-13T00:00:00Z".parse().unwrap(),
                )
                .unwrap();
        assert_eq!(exclude_newer, expected);
    }

    #[test]
    fn test_to_exclude_newer_absolute_cutoff() {
        for (cutoff, expected) in [
            ("2026-10-12", "2026-10-13T00:00:00Z"),
            ("2026-10-12T05:00:00Z", "2026-10-12T05:00:00Z"),
        ] {
            assert_eq!(
                rattler_solve::ExcludeNewer::try_from(ExcludeNewerConfig::from_cutoff(cutoff))
                    .unwrap(),
                rattler_solve::ExcludeNewer::from_datetime(expected.parse().unwrap()),
            );
        }
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
    fn test_parse_table_unknown_key() {
        // A typo such as `exemption` would otherwise drop every exemption.
        let error = toml::from_str::<Document>(
            r#"
            [exclude-newer]
            cutoff = "7d"
            exemption = { ruff = "*" }
            "#,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("unknown field `exemption`"),
            "{error}"
        );
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
            "[exclude-newer]\ncutoff = \"7d\"\n\n[exclude-newer.exemptions.ruff]\nversion = \">=0.5,<0.6\"\nbuild = \"py*\"\nchannel = \"conda-forge\"\nmd5 = \"00000000000000000000000000000000\"\ncutoff = \"2026-10-12\"\n",
            // A channel that its name doesn't resolve to is written as a URL.
            "[exclude-newer]\ncutoff = \"7d\"\n\n[exclude-newer.exemptions.ruff]\nchannel = \"https://prefix.dev/conda-forge/\"\n",
            "[exclude-newer]\ncutoff = \"7d\"\n\n[exclude-newer.exemptions.ruff]\nflags = [\"cuda*\"]\ntrack-features = [\"mkl\"]\n",
        ] {
            let document: Document = toml::from_str(input).unwrap();
            assert_eq!(toml::to_string(&document).unwrap(), input);
        }
    }
}
