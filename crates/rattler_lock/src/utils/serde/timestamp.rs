use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DeserializeAs, SerializeAs};

pub(crate) struct Timestamp;

impl<'de> DeserializeAs<'de, jiff::Timestamp> for Timestamp {
    fn deserialize_as<D>(deserializer: D) -> Result<jiff::Timestamp, D::Error>
    where
        D: Deserializer<'de>,
    {
        let timestamp = i64::deserialize(deserializer)?;

        // Determine if this is milliseconds or seconds based on magnitude
        if timestamp > 253_402_300_799 {
            // This is milliseconds (year 9999 in seconds is 253402300799)
            jiff::Timestamp::from_millisecond(timestamp)
                .map_err(|e| D::Error::custom(format!("got invalid timestamp: {e}")))
        } else {
            // This is seconds
            jiff::Timestamp::from_second(timestamp)
                .map_err(|e| D::Error::custom(format!("got invalid timestamp: {e}")))
        }
    }
}

impl SerializeAs<jiff::Timestamp> for Timestamp {
    fn serialize_as<S>(source: &jiff::Timestamp, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Convert the date to a timestamp in milliseconds
        let timestamp: i64 = source.as_millisecond();

        // Determine the precision of the timestamp.
        // If it's a round number of seconds, serialize as seconds
        let timestamp = if timestamp % 1000 == 0 {
            timestamp / 1000
        } else {
            timestamp
        };

        // Serialize the timestamp
        timestamp.serialize(serializer)
    }
}
