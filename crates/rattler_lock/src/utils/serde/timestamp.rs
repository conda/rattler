use jiff::Timestamp as JiffTimestamp;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DeserializeAs, SerializeAs};

/// Converts a raw integer timestamp (seconds or milliseconds) to a
/// `jiff::Timestamp`. Values larger than the year-9999 boundary in seconds
/// are treated as milliseconds; smaller values are treated as seconds.
pub(crate) fn millis_to_timestamp(timestamp: i64) -> Option<JiffTimestamp> {
    if timestamp > 253_402_300_799 {
        JiffTimestamp::from_millisecond(timestamp).ok()
    } else {
        JiffTimestamp::from_second(timestamp).ok()
    }
}

/// Converts a `jiff::Timestamp` to milliseconds since the Unix epoch.
pub(crate) fn timestamp_to_millis(ts: &JiffTimestamp) -> i64 {
    ts.as_millisecond()
}

pub(crate) struct Timestamp;

impl<'de> DeserializeAs<'de, JiffTimestamp> for Timestamp {
    fn deserialize_as<D>(deserializer: D) -> Result<JiffTimestamp, D::Error>
    where
        D: Deserializer<'de>,
    {
        let timestamp = i64::deserialize(deserializer)?;
        millis_to_timestamp(timestamp)
            .ok_or_else(|| D::Error::custom("got invalid timestamp, timestamp out of range"))
    }
}

impl SerializeAs<JiffTimestamp> for Timestamp {
    fn serialize_as<S>(source: &JiffTimestamp, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        timestamp_to_millis(source).serialize(serializer)
    }
}
