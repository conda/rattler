use chrono::{DateTime, Utc};
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DeserializeAs, SerializeAs};

/// Converts a raw integer timestamp (seconds or milliseconds) to a
/// `DateTime<Utc>`. Values larger than the year-9999 boundary in seconds
/// are treated as milliseconds; smaller values are treated as seconds.
pub(crate) fn millis_to_datetime(timestamp: i64) -> Option<DateTime<Utc>> {
    let microseconds = if timestamp > 253_402_300_799 {
        timestamp * 1_000
    } else {
        timestamp * 1_000_000
    };
    DateTime::from_timestamp_micros(microseconds)
}

/// Converts a `DateTime<Utc>` to milliseconds since the Unix epoch.
pub(crate) fn datetime_to_millis(dt: &DateTime<Utc>) -> i64 {
    dt.timestamp_millis()
}

pub(crate) struct Timestamp;

impl<'de> DeserializeAs<'de, chrono::DateTime<chrono::Utc>> for Timestamp {
    fn deserialize_as<D>(deserializer: D) -> Result<chrono::DateTime<chrono::Utc>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let timestamp = i64::deserialize(deserializer)?;
        millis_to_datetime(timestamp)
            .ok_or_else(|| D::Error::custom("got invalid timestamp, timestamp out of range"))
    }
}

impl SerializeAs<chrono::DateTime<chrono::Utc>> for Timestamp {
    fn serialize_as<S>(source: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        datetime_to_millis(source).serialize(serializer)
    }
}
