use chrono::{DateTime, Utc};
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DeserializeAs, SerializeAs};

pub(crate) struct Timestamp;

impl<'de> DeserializeAs<'de, chrono::DateTime<chrono::Utc>> for Timestamp {
    fn deserialize_as<D>(deserializer: D) -> Result<chrono::DateTime<chrono::Utc>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let timestamp = i64::deserialize(deserializer)?;

        // Convert from milliseconds to seconds
        let microseconds = if timestamp > 253_402_300_799 {
            timestamp * 1_000
        } else {
            timestamp * 1_000_000
        };

        // Convert the timestamp to a UTC timestamp
        chrono::DateTime::from_timestamp_micros(microseconds)
            .ok_or_else(|| D::Error::custom("got invalid timestamp, timestamp out of range"))
    }
}

impl SerializeAs<chrono::DateTime<chrono::Utc>> for Timestamp {
    fn serialize_as<S>(source: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Convert the date to a timestamp
        let timestamp: i64 = source.timestamp_millis();

        // Determine the precision of the timestamp.
        let timestamp = if timestamp % 1000 == 0 {
            timestamp / 1000
        } else {
            timestamp
        };

        // Serialize the timestamp
        timestamp.serialize(serializer)
    }
}
