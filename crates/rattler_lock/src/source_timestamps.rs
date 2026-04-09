use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rattler_conda_types::{ChannelUrl, PackageName};
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;

use crate::utils::serde::{datetime_to_millis, millis_to_datetime};

/// Timestamps associated with a source package.
///
/// Stores a default (global) timestamp, optional per-channel overrides, and
/// optional per-package overrides. When only a default timestamp is present the
/// value serializes as a plain integer (milliseconds since the Unix epoch) for
/// backwards compatibility. When any per-channel or per-package overrides are
/// present it serializes as a map:
///
/// ```yaml
/// timestamp:
///   default: 1699280294368
///   channels:
///     https://conda.anaconda.org/conda-forge: 1699280294000
///   packages:
///     numpy: null
/// ```
///
/// A `None` value inside the channels/packages maps indicates that the
/// channel/package is explicitly *not used* by this source package.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceTimestamps {
    /// The timestamp of the newest package used in the build or host environment.
    pub latest: DateTime<Utc>,

    /// Per-channel timestamp overrides keyed by channel URL.
    ///
    /// `Some(dt)` means use this timestamp for the channel; `None` means the
    /// channel is explicitly not used by this source package.
    pub channels: BTreeMap<ChannelUrl, Option<DateTime<Utc>>>,

    /// Per-package timestamp overrides keyed by package name.
    ///
    /// `Some(dt)` means use this timestamp for the package; `None` means the
    /// package is explicitly not used by this source package.
    pub packages: BTreeMap<PackageName, Option<DateTime<Utc>>>,
}

impl SourceTimestamps {
    /// Creates a `SourceTimestamps` with only a default timestamp.
    pub fn from_default(dt: DateTime<Utc>) -> Self {
        Self {
            latest: dt,
            channels: BTreeMap::new(),
            packages: BTreeMap::new(),
        }
    }

    /// Returns `true` when only the default timestamp is (possibly) set and
    /// there are no per-channel or per-package overrides.
    pub fn is_simple(&self) -> bool {
        self.channels.is_empty() && self.packages.is_empty()
    }

    /// Adds a per-channel timestamp override.
    pub fn with_channel(mut self, url: ChannelUrl, ts: Option<DateTime<Utc>>) -> Self {
        self.channels.insert(url, ts);
        self
    }

    /// Adds a per-package timestamp override.
    pub fn with_package(mut self, name: PackageName, ts: Option<DateTime<Utc>>) -> Self {
        self.packages.insert(name, ts);
        self
    }
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

impl Serialize for SourceTimestamps {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Simple case: only default timestamp, serialize as plain i64.
        if self.is_simple() {
            return datetime_to_millis(&self.latest).serialize(serializer);
        }

        // Complex case: serialize as map.
        let mut count = 1; // always have default
        if !self.channels.is_empty() {
            count += 1;
        }
        if !self.packages.is_empty() {
            count += 1;
        }

        let mut map = serializer.serialize_map(Some(count))?;
        map.serialize_entry("latest", &datetime_to_millis(&self.latest))?;

        if !self.channels.is_empty() {
            map.serialize_entry("channels", &OptionTimestampMap::Channels(&self.channels))?;
        }

        if !self.packages.is_empty() {
            map.serialize_entry("packages", &OptionTimestampMap::Packages(&self.packages))?;
        }

        map.end()
    }
}

/// Helper to serialize `BTreeMap<K, Option<DateTime<Utc>>>` where each value
/// is either milliseconds or `null`.
enum OptionTimestampMap<'a> {
    Channels(&'a BTreeMap<ChannelUrl, Option<DateTime<Utc>>>),
    Packages(&'a BTreeMap<PackageName, Option<DateTime<Utc>>>),
}

impl Serialize for OptionTimestampMap<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            OptionTimestampMap::Channels(map) => {
                let mut m = serializer.serialize_map(Some(map.len()))?;
                for (k, v) in *map {
                    m.serialize_entry(k.as_str(), &v.as_ref().map(datetime_to_millis))?;
                }
                m.end()
            }
            OptionTimestampMap::Packages(map) => {
                let mut m = serializer.serialize_map(Some(map.len()))?;
                for (k, v) in *map {
                    m.serialize_entry(k.as_normalized(), &v.as_ref().map(datetime_to_millis))?;
                }
                m.end()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Deserialization
// ---------------------------------------------------------------------------

impl<'de> Deserialize<'de> for SourceTimestamps {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(SourceTimestampsVisitor)
    }
}

struct SourceTimestampsVisitor;

impl<'de> Visitor<'de> for SourceTimestampsVisitor {
    type Value = SourceTimestamps;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an integer (milliseconds) or a map with default/channels/packages")
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        let dt = millis_to_datetime(v).ok_or_else(|| E::custom("timestamp out of range"))?;
        Ok(SourceTimestamps::from_default(dt))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        self.visit_i64(v as i64)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut latest: Option<DateTime<Utc>> = None;
        let mut channels: BTreeMap<ChannelUrl, Option<DateTime<Utc>>> = BTreeMap::new();
        let mut packages: BTreeMap<PackageName, Option<DateTime<Utc>>> = BTreeMap::new();

        while let Some(key) = map.next_key::<&str>()? {
            match key {
                "latest" => {
                    let millis: i64 = map.next_value()?;
                    latest = Some(
                        millis_to_datetime(millis)
                            .ok_or_else(|| de::Error::custom("default timestamp out of range"))?,
                    );
                }
                "channels" => {
                    let raw: BTreeMap<String, Option<i64>> = map.next_value()?;
                    for (url_str, ms) in raw {
                        let url: ChannelUrl =
                            Url::parse(&url_str).map_err(de::Error::custom)?.into();
                        let dt = ms
                            .map(|m| {
                                millis_to_datetime(m).ok_or_else(|| {
                                    de::Error::custom(format!(
                                        "channel timestamp out of range for {url_str}"
                                    ))
                                })
                            })
                            .transpose()?;
                        channels.insert(url, dt);
                    }
                }
                "packages" => {
                    let raw: BTreeMap<String, Option<i64>> = map.next_value()?;
                    for (name_str, ms) in raw {
                        let name = PackageName::new_unchecked(name_str);
                        let dt = ms
                            .map(|m| {
                                millis_to_datetime(m).ok_or_else(|| {
                                    de::Error::custom(format!(
                                        "package timestamp out of range for {}",
                                        name.as_normalized()
                                    ))
                                })
                            })
                            .transpose()?;
                        packages.insert(name, dt);
                    }
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        &["default", "channels", "packages"],
                    ));
                }
            }
        }

        let default = latest.ok_or_else(|| de::Error::missing_field("latest"))?;

        Ok(SourceTimestamps {
            latest: default,
            channels,
            packages,
        })
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

impl From<DateTime<Utc>> for SourceTimestamps {
    fn from(dt: DateTime<Utc>) -> Self {
        Self::from_default(dt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(millis: i64) -> DateTime<Utc> {
        millis_to_datetime(millis).unwrap()
    }

    #[test]
    fn simple_roundtrip_yaml() {
        let ts = SourceTimestamps::from_default(dt(1699280294368));
        let yaml = serde_yaml::to_string(&ts).unwrap();
        assert_eq!(yaml.trim(), "1699280294368");
        let back: SourceTimestamps = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(ts, back);
    }

    #[test]
    fn map_roundtrip_yaml() {
        let ts = SourceTimestamps {
            latest: dt(1699280294368),
            channels: BTreeMap::from([(
                ChannelUrl::from(Url::parse("https://conda.anaconda.org/conda-forge/").unwrap()),
                Some(dt(1699280294000)),
            )]),
            packages: BTreeMap::from([(PackageName::new_unchecked("numpy".to_string()), None)]),
        };
        let yaml = serde_yaml::to_string(&ts).unwrap();
        assert!(yaml.contains("latest:"));
        assert!(yaml.contains("channels:"));
        assert!(yaml.contains("packages:"));
        assert!(yaml.contains("null"));
        let back: SourceTimestamps = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(ts, back);
    }

    #[test]
    fn deserialize_plain_integer() {
        let ts: SourceTimestamps = serde_yaml::from_str("1699280294368").unwrap();
        assert!(ts.is_simple());
        assert_eq!(ts.latest, dt(1699280294368));
    }

    #[test]
    fn is_simple() {
        let simple = SourceTimestamps::from_default(dt(1000));
        assert!(simple.is_simple());

        let complex = simple.with_package(
            PackageName::new_unchecked("foo".to_string()),
            Some(dt(2000)),
        );
        assert!(!complex.is_simple());
    }
}
