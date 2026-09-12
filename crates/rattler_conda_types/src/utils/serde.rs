//! Serde utilities for conda types.

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::Error};
use serde_with::{DeserializeAs, SerializeAs};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::{
    marker::PhantomData,
    path::{Path, PathBuf},
};
use url::Url;

/// A helper struct that serializes Paths in a normalized way.
/// - Backslashes are replaced with forward-slashes.
pub(crate) struct NormalizedPath;

impl<P: AsRef<Path>> SerializeAs<P> for NormalizedPath {
    fn serialize_as<S>(source: &P, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match source.as_ref().to_str() {
            Some(s) => s.replace('\\', "/").serialize(serializer),
            None => Err(S::Error::custom("path contains invalid UTF-8 characters")),
        }
    }
}

impl<'de> DeserializeAs<'de, PathBuf> for NormalizedPath {
    fn deserialize_as<D>(deserializer: D) -> Result<PathBuf, D::Error>
    where
        D: Deserializer<'de>,
    {
        PathBuf::deserialize(deserializer)
    }
}

/// Deserialize a sequence into `Vec<T>` but filter `None` values.
pub(crate) struct VecSkipNone<T>(PhantomData<T>);

impl<'de, T, I> DeserializeAs<'de, Vec<T>> for VecSkipNone<I>
where
    I: DeserializeAs<'de, Vec<Option<T>>>,
{
    fn deserialize_as<D>(deserializer: D) -> Result<Vec<T>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(I::deserialize_as(deserializer)?
            .into_iter()
            .flatten()
            .collect())
    }
}

/// A helper type parser that tries to parse Urls that could be malformed.
pub(crate) struct LossyUrl;

impl<'de> DeserializeAs<'de, Option<Url>> for LossyUrl {
    fn deserialize_as<D>(deserializer: D) -> Result<Option<Url>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let str = match Option::<String>::deserialize(deserializer)? {
            Some(url) => url,
            None => return Ok(None),
        };
        let url = match Url::parse(&str) {
            Ok(url) => url,
            Err(e) => {
                tracing::warn!("unable to parse '{}' as an URL: {e}. Skipping...", str);
                return Ok(None);
            }
        };
        Ok(Some(url))
    }
}

/// A helper type that parses a string either as a string or a vector of
/// strings.
pub(crate) struct MultiLineString;

impl<'de> DeserializeAs<'de, String> for MultiLineString {
    fn deserialize_as<D>(deserializer: D) -> Result<String, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Inner {
            String(String),
            Multi(Vec<String>),
        }

        Ok(match Inner::deserialize(deserializer)? {
            Inner::String(s) => s,
            Inner::Multi(s) => s.join("\n"),
        })
    }
}

/// Wrapper type for timestamps that preserves whether they were originally
/// in seconds or milliseconds format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimestampMs {
    timestamp: jiff::Timestamp,
    /// Whether the original timestamp was in milliseconds (true) or seconds (false)
    is_millis: bool,
}

impl TimestampMs {
    /// Create a new `TimestampMs` from a `Timestamp` with millisecond precision
    pub fn from_timestamp_millis(timestamp: jiff::Timestamp) -> Self {
        Self {
            timestamp,
            is_millis: true,
        }
    }

    /// Create a new `TimestampMs` from a `Timestamp` with second precision
    pub fn from_timestamp_seconds(timestamp: jiff::Timestamp) -> Self {
        Self {
            timestamp,
            is_millis: false,
        }
    }

    /// Get the inner `Timestamp`
    pub fn jiff_timestamp(&self) -> jiff::Timestamp {
        self.timestamp
    }

    /// Get the timestamp as seconds since Unix epoch
    pub fn timestamp(&self) -> i64 {
        self.timestamp.as_second()
    }

    /// Get the timestamp as milliseconds since Unix epoch
    pub fn timestamp_millis(&self) -> i64 {
        self.timestamp.as_millisecond()
    }
}

impl PartialOrd for TimestampMs {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TimestampMs {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.timestamp.cmp(&other.timestamp)
    }
}

// Allow comparison with jiff::Timestamp
impl PartialEq<jiff::Timestamp> for TimestampMs {
    fn eq(&self, other: &jiff::Timestamp) -> bool {
        self.timestamp == *other
    }
}

impl PartialOrd<jiff::Timestamp> for TimestampMs {
    fn partial_cmp(&self, other: &jiff::Timestamp) -> Option<std::cmp::Ordering> {
        self.timestamp.partial_cmp(other)
    }
}

impl From<jiff::Timestamp> for TimestampMs {
    fn from(timestamp: jiff::Timestamp) -> Self {
        // Default to millisecond precision for compatibility
        Self::from_timestamp_millis(timestamp)
    }
}

impl From<TimestampMs> for jiff::Timestamp {
    fn from(ts: TimestampMs) -> Self {
        ts.timestamp
    }
}

impl<'de> Deserialize<'de> for TimestampMs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let timestamp = i64::deserialize(deserializer)?;

        // Determine if this is milliseconds or seconds based on magnitude
        let (ts, is_millis) = if timestamp > 253_402_300_799 {
            // This is milliseconds (year 9999 in seconds is 253402300799)
            let ts = jiff::Timestamp::from_millisecond(timestamp).map_err(|_err| {
                D::Error::custom("got invalid timestamp, timestamp out of range")
            })?;
            (ts, true)
        } else {
            // This is seconds
            let ts = jiff::Timestamp::from_second(timestamp).map_err(|_err| {
                D::Error::custom("got invalid timestamp, timestamp out of range")
            })?;
            (ts, false)
        };

        Ok(Self {
            timestamp: ts,
            is_millis,
        })
    }
}

impl Serialize for TimestampMs {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Preserve the original format
        let timestamp = if self.is_millis {
            self.timestamp.as_millisecond()
        } else {
            self.timestamp.as_second()
        };

        timestamp.serialize(serializer)
    }
}

/// A helper struct to deserialize types from a string without checking the
/// string.
pub struct DeserializeFromStrUnchecked;

/// A helper struct to deserialize virtual package plugin registrations,
/// validating every name and skipping the ones a channel got wrong.
#[cfg(feature = "experimental-virtual-package-plugins")]
pub struct DeserializeVirtualPackagePlugins;

#[cfg(feature = "experimental-virtual-package-plugins")]
impl<'de> DeserializeAs<'de, crate::repo_data::VirtualPackagePlugins>
    for DeserializeVirtualPackagePlugins
{
    fn deserialize_as<D>(
        deserializer: D,
    ) -> Result<crate::repo_data::VirtualPackagePlugins, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Read the entries as pairs rather than a map: a map resolves a
        // repeated key last-wins, which would hide a collision the CEP makes an
        // error.
        struct Entries;
        impl<'de> serde::de::Visitor<'de> for Entries {
            type Value = Option<Vec<(String, Vec<MaybeName>)>>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a map of plugin names to virtual package names")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry::<String, Vec<MaybeName>>()? {
                    entries.push(entry);
                }
                Ok(Some(entries))
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }
        }

        let raw = match deserializer.deserialize_any(Entries) {
            Ok(Some(raw)) => raw,
            Ok(None) => {
                tracing::warn!(
                    "ignoring info.virtual_package_plugins: it is not a map of plugin registrations"
                );
                return Ok(crate::repo_data::VirtualPackagePlugins::default());
            }
            Err(reason) => {
                tracing::warn!("ignoring info.virtual_package_plugins: {reason}");
                return Ok(crate::repo_data::VirtualPackagePlugins::default());
            }
        };

        // A channel that contradicted itself has not established what it meant,
        // so the whole set goes rather than the offending entry.
        match registrations_from(raw) {
            Ok(registrations) => Ok(registrations),
            Err(reason) => {
                tracing::warn!("ignoring info.virtual_package_plugins entirely: {reason}");
                Ok(crate::repo_data::VirtualPackagePlugins::default())
            }
        }
    }
}

/// Builds the registrations, or names the contradiction that makes the whole
/// set unusable. Returning the reason rather than a value lets the caller report
/// it and fall back to no registrations.
#[cfg(feature = "experimental-virtual-package-plugins")]
fn registrations_from(
    raw: Vec<(String, Vec<MaybeName>)>,
) -> Result<crate::repo_data::VirtualPackagePlugins, String> {
    let mut registrations = crate::repo_data::VirtualPackagePlugins::default();
    let mut registered_plugins = std::collections::HashSet::new();
    let mut claimed = std::collections::HashSet::new();

    for (plugin, provides) in raw {
        let plugin = validated_plugin_name(plugin)?;
        if !registered_plugins.insert(plugin.clone()) {
            return Err(format!(
                "the channel registers the plugin package '{}' more than once",
                plugin.as_source()
            ));
        }

        // Counted over what the plugin declares, before invalid names are dropped.
        if provides.is_empty() || provides.len() > MAX_VIRTUAL_PACKAGES_PER_PLUGIN {
            return Err(format!(
                "plugin '{}' registers {} virtual packages, outside the 1 to {} a plugin may \
                 register",
                plugin.as_source(),
                provides.len(),
                MAX_VIRTUAL_PACKAGES_PER_PLUGIN
            ));
        }

        let provides: Vec<_> = provides
            .into_iter()
            .filter_map(|name| match name {
                MaybeName::Name(name) => validated_virtual_package_name(name),
                MaybeName::NotAName(_) => {
                    tracing::warn!(
                        "ignoring a registered virtual package of plugin '{}': it is not a name \
                         at all",
                        plugin.as_source()
                    );
                    None
                }
            })
            .collect();
        for name in &provides {
            if !claimed.insert(name.clone()) {
                return Err(format!(
                    "the virtual package '{}' is registered more than once",
                    name.as_source()
                ));
            }
        }

        // Every name it declared was invalid and dropped above. The plugin
        // speaks for nothing, which is ignored rather than fatal.
        if provides.is_empty() {
            tracing::warn!(
                "ignoring plugin '{}': it registers no valid virtual package",
                plugin.as_source()
            );
            continue;
        }

        registrations.insert(plugin, provides);
    }
    Ok(registrations)
}

/// One element of a registration array. A channel that put something other than
/// a string there has published a name that cannot be a package name, and the
/// CEP requires such a value to be discarded
#[cfg(feature = "experimental-virtual-package-plugins")]
#[derive(Deserialize)]
#[serde(untagged)]
enum MaybeName {
    Name(String),
    NotAName(serde::de::IgnoredAny),
}

/// The longest name CEP 26 allows, for a package or a virtual package.
#[cfg(feature = "experimental-virtual-package-plugins")]
const MAX_NAME_LENGTH: usize = 64;

/// The most virtual packages CEP lets one plugin register, which bounds how much
/// a single plugin can feed into a solve.
#[cfg(feature = "experimental-virtual-package-plugins")]
const MAX_VIRTUAL_PACKAGES_PER_PLUGIN: usize = 16;

/// The distributable package name rule of CEP 26. `fancy_regex` is used
/// because the pattern needs the negative lookahead that keeps a leading
/// underscore from being followed by another.
#[cfg(feature = "experimental-virtual-package-plugins")]
static PACKAGE_NAME: std::sync::LazyLock<fancy_regex::Regex> = std::sync::LazyLock::new(|| {
    #[allow(clippy::expect_used, reason = "the pattern is a literal from CEP 26")]
    fancy_regex::Regex::new(r"(?i)^(([a-z0-9])|([a-z0-9_](?!_)))[._-]?([a-z0-9]+(\.|-|_|$))*$")
        .expect("the CEP 26 package name pattern is valid")
});

/// The virtual package name rule of CEP 26.
#[cfg(feature = "experimental-virtual-package-plugins")]
static VIRTUAL_PACKAGE_NAME: std::sync::LazyLock<fancy_regex::Regex> =
    std::sync::LazyLock::new(|| {
        #[allow(clippy::expect_used, reason = "the pattern is a literal from CEP 26")]
        fancy_regex::Regex::new(r"^__[a-z0-9][._-]?([a-z0-9]+(\.|-|_|$))*$")
            .expect("the CEP 26 virtual package name pattern is valid")
    });

/// Whether a name a channel published satisfies one of the CEP 26 patterns.
#[cfg(feature = "experimental-virtual-package-plugins")]
fn matches(pattern: &fancy_regex::Regex, name: &crate::PackageName) -> bool {
    let name = name.as_source();
    name.len() <= MAX_NAME_LENGTH && pattern.is_match(name).unwrap_or(false)
}

/// Validates the name of the package providing a plugin.
#[cfg(feature = "experimental-virtual-package-plugins")]
fn validated_plugin_name(name: String) -> Result<crate::PackageName, String> {
    let source = name.clone();
    let name = crate::PackageName::try_from(name)
        .map_err(|err| format!("'{source}' is not a package name: {err}"))?;
    if !matches(&PACKAGE_NAME, &name) {
        return Err(format!("'{}' is not a package name", name.as_source()));
    }
    Ok(name)
}

/// Validates a name a plugin claims to provide, which CEP 26 requires to be a
/// package name carrying the `__` prefix.
#[cfg(feature = "experimental-virtual-package-plugins")]
fn validated_virtual_package_name(name: String) -> Option<crate::PackageName> {
    let name = crate::PackageName::try_from(name)
        .inspect_err(|err| tracing::warn!("ignoring registered virtual package name: {err}"))
        .ok()?;
    if !matches(&VIRTUAL_PACKAGE_NAME, &name) {
        tracing::warn!(
            "ignoring registered virtual package '{}': it is not a virtual package name",
            name.as_source()
        );
        return None;
    }
    Some(name)
}

/// A helper function used to sort map alphabetically when serializing.
pub(crate) fn sort_map_alphabetically<K: Ord + Serialize, T: Serialize, H, S: serde::Serializer>(
    value: &HashMap<K, T, H>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value
        .iter()
        .collect::<BTreeMap<_, _>>()
        .serialize(serializer)
}

/// A helper function used to sort map alphabetically when serializing.
pub(crate) fn sort_index_map_alphabetically<
    K: Ord + Serialize,
    T: Serialize,
    H,
    S: serde::Serializer,
>(
    value: &IndexMap<K, T, H>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value
        .iter()
        .collect::<BTreeMap<_, _>>()
        .serialize(serializer)
}

/// A helper function used to sort a set alphabetically when serializing.
pub(crate) fn sort_set_alphabetically<K: Ord + Serialize, S: serde::Serializer>(
    value: &ahash::HashSet<K>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value.iter().collect::<BTreeSet<_>>().serialize(serializer)
}

/// A helper to serialize and deserialize `track_features` in repodata. Track
/// features are expected to be a space separated list. However, in the past we
/// have serialized and deserialized them as a list of strings so for
/// deserialization that behavior is retained.
pub struct Features;

impl SerializeAs<Vec<String>> for Features {
    fn serialize_as<S>(source: &Vec<String>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        source.join(" ").serialize(serializer)
    }
}

impl<'de> DeserializeAs<'de, Vec<String>> for Features {
    fn deserialize_as<D>(deserializer: D) -> Result<Vec<String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .expecting("a string or a sequence of strings")
            .string(|str| {
                Ok(str
                    .split([',', ' '])
                    .map(str::trim)
                    .map(String::from)
                    .collect())
            })
            .seq(|seq| {
                let vec: Vec<Cow<'de, str>> = seq.deserialize()?;
                Ok(vec
                    .iter()
                    .map(Cow::as_ref)
                    .map(str::trim)
                    .map(String::from)
                    .collect())
            })
            .deserialize(deserializer)
    }
}

pub fn is_none_or_empty_string(opt: &Option<String>) -> bool {
    opt.as_ref().is_none_or(String::is_empty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timestamp_ms_preserves_seconds() {
        // Test a timestamp in seconds (1640000000 = 2021-12-20)
        let json = "1640000000";
        let ts: TimestampMs = serde_json::from_str(json).unwrap();

        // Verify it was recognized as seconds
        assert!(!ts.is_millis);

        // Verify it round-trips correctly
        let serialized = serde_json::to_string(&ts).unwrap();
        assert_eq!(serialized, json);
    }

    #[test]
    fn test_timestamp_ms_preserves_milliseconds() {
        // Test a timestamp in milliseconds (1640000000000 = 2021-12-20)
        let json = "1640000000000";
        let ts: TimestampMs = serde_json::from_str(json).unwrap();

        // Verify it was recognized as milliseconds
        assert!(ts.is_millis);

        // Verify it round-trips correctly
        let serialized = serde_json::to_string(&ts).unwrap();
        assert_eq!(serialized, json);
    }

    #[test]
    fn test_timestamp_ms_milliseconds_ending_with_000() {
        // Test a timestamp in milliseconds that ends with 000
        // This was the problematic case in the old implementation
        let json = "1640000000000"; // 2021-12-20 00:00:00.000
        let ts: TimestampMs = serde_json::from_str(json).unwrap();

        // Verify it was recognized as milliseconds
        assert!(ts.is_millis);

        // Verify it serializes back to milliseconds (not seconds)
        let serialized = serde_json::to_string(&ts).unwrap();
        assert_eq!(serialized, json);
    }

    #[test]
    fn test_timestamp_ms_seconds_ending_with_000() {
        // Test a timestamp in seconds that ends with 000
        let json = "1640000000"; // 2021-12-20 00:00:00
        let ts: TimestampMs = serde_json::from_str(json).unwrap();

        // Verify it was recognized as seconds
        assert!(!ts.is_millis);

        // Verify it serializes back to seconds
        let serialized = serde_json::to_string(&ts).unwrap();
        assert_eq!(serialized, json);
    }

    #[test]
    fn test_timestamp_ms_from_timestamp() {
        let timestamp = jiff::Timestamp::from_second(1640000000).unwrap();

        // Test creating from timestamp with milliseconds precision marker
        let ts_millis = TimestampMs::from_timestamp_millis(timestamp);
        assert_eq!(ts_millis.jiff_timestamp(), timestamp);
        // Serializes as milliseconds
        let json = serde_json::to_string(&ts_millis).unwrap();
        assert_eq!(json, "1640000000000");

        // Test creating from timestamp with seconds precision marker
        let ts_seconds = TimestampMs::from_timestamp_seconds(timestamp);
        assert_eq!(ts_seconds.jiff_timestamp(), timestamp);
        // Serializes as seconds
        let json = serde_json::to_string(&ts_seconds).unwrap();
        assert_eq!(json, "1640000000");
    }

    #[test]
    fn test_timestamp_ms_conversion() {
        let timestamp = jiff::Timestamp::from_second(1640000000).unwrap();

        // Test From trait
        let ts: TimestampMs = timestamp.into();
        assert_eq!(ts.jiff_timestamp(), timestamp);

        // Test Into trait
        let converted: jiff::Timestamp = ts.into();
        assert_eq!(converted, timestamp);
    }
}
