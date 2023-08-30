use chrono::{DateTime, Utc};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::de::DeserializeAsWrap;
use serde_with::ser::SerializeAsWrap;
use serde_with::{DeserializeAs, SerializeAs};
use std::collections::HashSet;
use std::hash::{BuildHasher, Hash};
use std::marker::PhantomData;
use url::Url;

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

/// A helper type that parses a string either as a string or a vector of strings.
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
        Ok(chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
            chrono::NaiveDateTime::from_timestamp_micros(microseconds)
                .ok_or_else(|| D::Error::custom("got invalid timestamp, timestamp out of range"))?,
            chrono::Utc,
        ))
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

/// Used with serde_with to serialize a collection as a sorted collection.
#[derive(Default)]
pub(crate) struct Ordered<T>(PhantomData<T>);

impl<'de, T: Eq + Hash, S: BuildHasher + Default, TAs> DeserializeAs<'de, HashSet<T, S>>
    for Ordered<TAs>
where
    TAs: DeserializeAs<'de, T>,
{
    fn deserialize_as<D>(deserializer: D) -> Result<HashSet<T, S>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let content =
            DeserializeAsWrap::<Vec<T>, Vec<TAs>>::deserialize(deserializer)?.into_inner();
        Ok(HashSet::from_iter(content.into_iter()))
    }
}

impl<T: Ord, HS, TAs: SerializeAs<T>> SerializeAs<HashSet<T, HS>> for Ordered<TAs> {
    fn serialize_as<S>(source: &HashSet<T, HS>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut elements = Vec::from_iter(source.iter());
        elements.sort();
        SerializeAsWrap::<Vec<&T>, Vec<&TAs>>::new(&elements).serialize(serializer)
    }
}

impl<'de, T: Ord, TAs> DeserializeAs<'de, Vec<T>> for Ordered<TAs>
where
    TAs: DeserializeAs<'de, T>,
{
    fn deserialize_as<D>(deserializer: D) -> Result<Vec<T>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut content =
            DeserializeAsWrap::<Vec<T>, Vec<TAs>>::deserialize(deserializer)?.into_inner();
        content.sort();
        Ok(content)
    }
}

impl<T: Ord, TAs: SerializeAs<T>> SerializeAs<Vec<T>> for Ordered<TAs> {
    fn serialize_as<S>(source: &Vec<T>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut elements = Vec::from_iter(source.iter());
        elements.sort();
        SerializeAsWrap::<Vec<&T>, Vec<&TAs>>::new(&elements).serialize(serializer)
    }
}

/// A helper struct to deserialize types from a string without checking the string.
pub struct DeserializeFromStrUnchecked;
