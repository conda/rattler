use std::borrow::Cow;
use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rattler_conda_types::{ChannelUrl, PackageName};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_untagged::UntaggedEnumVisitor;
use serde_with::serde_as;

use crate::utils::serde::{datetime_to_millis, millis_to_datetime, Timestamp};

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
///   latest: 1699280294368
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
// Serialization / Deserialization
// ---------------------------------------------------------------------------

/// Helper struct mirroring the map form of [`SourceTimestamps`].
///
/// Acts as the single source of truth for the map-shaped wire format used by
/// both serialization and deserialization. The `Cow` fields allow
/// serialization to borrow from a live `SourceTimestamps` without cloning,
/// while deserialization produces owned data.
#[serde_as]
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceTimestampsMap<'a> {
    #[serde_as(as = "Timestamp")]
    latest: DateTime<Utc>,

    #[serde(
        default,
        skip_serializing_if = "cow_btree_is_empty",
        with = "optional_millis_map"
    )]
    channels: Cow<'a, BTreeMap<ChannelUrl, Option<DateTime<Utc>>>>,

    #[serde(
        default,
        skip_serializing_if = "cow_btree_is_empty",
        with = "optional_millis_map"
    )]
    packages: Cow<'a, BTreeMap<PackageName, Option<DateTime<Utc>>>>,
}

#[allow(clippy::ptr_arg)] // signature is required by `#[serde(skip_serializing_if = ...)]`
fn cow_btree_is_empty<K: Clone, V: Clone>(map: &Cow<'_, BTreeMap<K, V>>) -> bool {
    map.is_empty()
}

impl From<SourceTimestampsMap<'_>> for SourceTimestamps {
    fn from(value: SourceTimestampsMap<'_>) -> Self {
        Self {
            latest: value.latest,
            channels: value.channels.into_owned(),
            packages: value.packages.into_owned(),
        }
    }
}

impl Serialize for SourceTimestamps {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Simple case: only the `latest` timestamp is set; serialize as a
        // plain integer (milliseconds) for backwards compatibility.
        if self.is_simple() {
            return datetime_to_millis(&self.latest).serialize(serializer);
        }

        // Complex case: delegate to the helper struct, borrowing the
        // channel/package maps to avoid cloning.
        SourceTimestampsMap {
            latest: self.latest,
            channels: Cow::Borrowed(&self.channels),
            packages: Cow::Borrowed(&self.packages),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SourceTimestamps {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        UntaggedEnumVisitor::new()
            .expecting("an integer (milliseconds) or a map with latest/channels/packages")
            .i64(|v| {
                millis_to_datetime(v)
                    .map(SourceTimestamps::from_default)
                    .ok_or_else(|| serde_untagged::de::Error::custom("timestamp out of range"))
            })
            .u64(|v| {
                let v = i64::try_from(v).map_err(serde_untagged::de::Error::custom)?;
                millis_to_datetime(v)
                    .map(SourceTimestamps::from_default)
                    .ok_or_else(|| serde_untagged::de::Error::custom("timestamp out of range"))
            })
            .map(|map| map.deserialize::<SourceTimestampsMap<'_>>().map(Into::into))
            .deserialize(deserializer)
    }
}

/// Serialization helpers for `Cow<'_, BTreeMap<K, Option<DateTime<Utc>>>>`
/// where the timestamp values are stored on the wire as milliseconds since
/// the Unix epoch (or `null`).
mod optional_millis_map {
    use std::borrow::Cow;
    use std::collections::BTreeMap;

    use chrono::{DateTime, Utc};
    use serde::de::Error as _;
    use serde::ser::SerializeMap;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use crate::utils::serde::{datetime_to_millis, millis_to_datetime};

    type TimestampMap<K> = BTreeMap<K, Option<DateTime<Utc>>>;

    #[allow(clippy::ptr_arg)] // signature is required by `#[serde(with = ...)]`
    pub(super) fn serialize<S, K>(
        map: &Cow<'_, TimestampMap<K>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        K: Serialize + Ord + Clone,
    {
        let mut m = serializer.serialize_map(Some(map.len()))?;
        for (key, value) in map.iter() {
            m.serialize_entry(key, &value.as_ref().map(datetime_to_millis))?;
        }
        m.end()
    }

    pub(super) fn deserialize<'de, 'a, D, K>(
        deserializer: D,
    ) -> Result<Cow<'a, TimestampMap<K>>, D::Error>
    where
        D: Deserializer<'de>,
        K: Deserialize<'de> + Ord + Clone,
    {
        let raw = BTreeMap::<K, Option<i64>>::deserialize(deserializer)?;
        let mut out = BTreeMap::new();
        for (key, value) in raw {
            let dt = match value {
                Some(millis) => Some(
                    millis_to_datetime(millis)
                        .ok_or_else(|| D::Error::custom("timestamp out of range"))?,
                ),
                None => None,
            };
            out.insert(key, dt);
        }
        Ok(Cow::Owned(out))
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
    use url::Url;

    use super::*;

    fn dt(millis: i64) -> DateTime<Utc> {
        millis_to_datetime(millis).unwrap()
    }

    fn channel(url: &str) -> ChannelUrl {
        ChannelUrl::from(Url::parse(url).unwrap())
    }

    fn pkg(name: &str) -> PackageName {
        PackageName::new_unchecked(name.to_string())
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
                channel("https://conda.anaconda.org/conda-forge/"),
                Some(dt(1699280294000)),
            )]),
            packages: BTreeMap::from([(pkg("numpy"), None)]),
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

        let complex = simple.with_package(pkg("foo"), Some(dt(2000)));
        assert!(!complex.is_simple());
    }

    #[test]
    fn deserialize_map_form() {
        let yaml = r#"
latest: 1699280294368
channels:
  https://conda.anaconda.org/conda-forge: 1699280294000
  https://conda.anaconda.org/bioconda: null
packages:
  numpy: 1699280200000
  scipy: null
"#;
        let ts: SourceTimestamps = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(ts.latest, dt(1699280294368));
        assert_eq!(
            ts.channels[&channel("https://conda.anaconda.org/conda-forge/")],
            Some(dt(1699280294000))
        );
        assert_eq!(
            ts.channels[&channel("https://conda.anaconda.org/bioconda/")],
            None
        );
        assert_eq!(ts.packages[&pkg("numpy")], Some(dt(1699280200000)));
        assert_eq!(ts.packages[&pkg("scipy")], None);
    }

    #[test]
    fn deserialize_map_form_missing_channels_packages_uses_defaults() {
        let yaml = "latest: 1699280294368\n";
        let ts: SourceTimestamps = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(ts.latest, dt(1699280294368));
        assert!(ts.is_simple());
    }

    #[test]
    fn deserialize_map_form_unknown_field_is_rejected() {
        let yaml = "latest: 1\nunknown: 2\n";
        let err = serde_yaml::from_str::<SourceTimestamps>(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown"), "error was: {msg}");
    }

    #[test]
    fn map_roundtrip_multiple_channels_and_packages() {
        let ts = SourceTimestamps {
            latest: dt(1699280294368),
            channels: BTreeMap::from([
                (
                    channel("https://conda.anaconda.org/conda-forge/"),
                    Some(dt(1699280294000)),
                ),
                (channel("https://conda.anaconda.org/bioconda/"), None),
            ]),
            packages: BTreeMap::from([
                (pkg("numpy"), Some(dt(1699280200000))),
                (pkg("scipy"), None),
            ]),
        };
        let yaml = serde_yaml::to_string(&ts).unwrap();
        let back: SourceTimestamps = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(ts, back);
    }
}
