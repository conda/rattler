//! Serde utilities for conda types.
use serde::{de::Error as _, ser::Error, Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DeserializeAs, SerializeAs};
use std::borrow::Cow;
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
    datetime: chrono::DateTime<chrono::Utc>,
    /// Whether the original timestamp was in milliseconds (true) or seconds (false)
    is_millis: bool,
}

impl TimestampMs {
    /// Create a new `TimestampMs` from a `DateTime` with millisecond precision
    pub fn from_datetime_millis(datetime: chrono::DateTime<chrono::Utc>) -> Self {
        Self {
            datetime,
            is_millis: true,
        }
    }

    /// Create a new `TimestampMs` from a `DateTime` with second precision
    pub fn from_datetime_seconds(datetime: chrono::DateTime<chrono::Utc>) -> Self {
        Self {
            datetime,
            is_millis: false,
        }
    }

    /// Get the inner `DateTime`
    pub fn datetime(&self) -> &chrono::DateTime<chrono::Utc> {
        &self.datetime
    }

    /// Convert to the inner `DateTime`
    pub fn into_datetime(self) -> chrono::DateTime<chrono::Utc> {
        self.datetime
    }

    /// Get the timestamp as seconds since Unix epoch
    pub fn timestamp(&self) -> i64 {
        self.datetime.timestamp()
    }

    /// Get the timestamp as milliseconds since Unix epoch
    pub fn timestamp_millis(&self) -> i64 {
        self.datetime.timestamp_millis()
    }
}

impl PartialOrd for TimestampMs {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TimestampMs {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.datetime.cmp(&other.datetime)
    }
}

// Allow comparison with DateTime<Utc>
impl PartialEq<chrono::DateTime<chrono::Utc>> for TimestampMs {
    fn eq(&self, other: &chrono::DateTime<chrono::Utc>) -> bool {
        self.datetime == *other
    }
}

impl PartialOrd<chrono::DateTime<chrono::Utc>> for TimestampMs {
    fn partial_cmp(&self, other: &chrono::DateTime<chrono::Utc>) -> Option<std::cmp::Ordering> {
        self.datetime.partial_cmp(other)
    }
}

impl From<chrono::DateTime<chrono::Utc>> for TimestampMs {
    fn from(datetime: chrono::DateTime<chrono::Utc>) -> Self {
        // Default to millisecond precision for compatibility
        Self::from_datetime_millis(datetime)
    }
}

impl From<TimestampMs> for chrono::DateTime<chrono::Utc> {
    fn from(ts: TimestampMs) -> Self {
        ts.datetime
    }
}

impl<'de> Deserialize<'de> for TimestampMs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let timestamp = i64::deserialize(deserializer)?;

        // Determine if this is milliseconds or seconds based on magnitude
        let (datetime, is_millis) = if timestamp > 253_402_300_799 {
            // This is milliseconds (year 9999 in seconds is 253402300799)
            let microseconds = timestamp * 1_000;
            let dt = chrono::DateTime::from_timestamp_micros(microseconds)
                .ok_or_else(|| D::Error::custom("got invalid timestamp, timestamp out of range"))?;
            (dt, true)
        } else {
            // This is seconds
            let microseconds = timestamp * 1_000_000;
            let dt = chrono::DateTime::from_timestamp_micros(microseconds)
                .ok_or_else(|| D::Error::custom("got invalid timestamp, timestamp out of range"))?;
            (dt, false)
        };

        Ok(Self { datetime, is_millis })
    }
}

impl Serialize for TimestampMs {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Preserve the original format
        let timestamp = if self.is_millis {
            self.datetime.timestamp_millis()
        } else {
            self.datetime.timestamp()
        };

        timestamp.serialize(serializer)
    }
}

/// A helper struct to deserialize types from a string without checking the
/// string.
pub struct DeserializeFromStrUnchecked;

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
    fn test_timestamp_ms_from_datetime() {
        let datetime = chrono::DateTime::from_timestamp(1640000000, 0).unwrap();

        // Test creating from datetime with milliseconds
        let ts_millis = TimestampMs::from_datetime_millis(datetime);
        assert!(ts_millis.is_millis);
        assert_eq!(ts_millis.datetime(), &datetime);

        // Test creating from datetime with seconds
        let ts_seconds = TimestampMs::from_datetime_seconds(datetime);
        assert!(!ts_seconds.is_millis);
        assert_eq!(ts_seconds.datetime(), &datetime);
    }

    #[test]
    fn test_timestamp_ms_conversion() {
        let datetime = chrono::DateTime::from_timestamp(1640000000, 0).unwrap();

        // Test From trait
        let ts: TimestampMs = datetime.into();
        assert!(ts.is_millis); // Default is milliseconds

        // Test Into trait
        let converted: chrono::DateTime<chrono::Utc> = ts.into();
        assert_eq!(converted, datetime);
    }
}
