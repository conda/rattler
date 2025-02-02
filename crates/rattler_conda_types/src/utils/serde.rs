use chrono::{DateTime, Utc};
use fxhash::FxHashMap;
use serde::{de::Error as _, ser::Error, Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DeserializeAs, SerializeAs};
use std::borrow::Cow;
use std::{
    collections::BTreeMap,
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

/// A helper struct to deserialize types from a string without checking the
/// string.
pub struct DeserializeFromStrUnchecked;

/// A helper function used to sort map alphabetically when serializing.
pub(crate) fn sort_map_alphabetically<T: Serialize, S: serde::Serializer>(
    value: &FxHashMap<String, T>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value
        .iter()
        .collect::<BTreeMap<_, _>>()
        .serialize(serializer)
}

/// A helper to serialize and deserialize `track_features` in repodata. Track
/// features are expected to be a space seperated list. However, in the past we
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
