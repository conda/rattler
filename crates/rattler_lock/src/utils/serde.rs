use chrono::{DateTime, Utc};
use fxhash::FxBuildHasher;
use indexmap::IndexMap;
use pep508_rs::VersionOrUrl;
use rattler_conda_types::MatchSpec;
use rattler_conda_types::{NamelessMatchSpec, PackageName};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::de::DeserializeAsWrap;
use serde_with::ser::SerializeAsWrap;
use serde_with::DisplayFromStr;
use serde_with::{serde_as, DeserializeAs, SerializeAs};
use std::collections::HashSet;
use std::hash::{BuildHasher, Hash};
use std::marker::PhantomData;

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
        Ok(HashSet::from_iter(content))
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

pub(crate) struct MatchSpecMapOrVec;

impl<'de> DeserializeAs<'de, Vec<String>> for MatchSpecMapOrVec {
    fn deserialize_as<D>(deserializer: D) -> Result<Vec<String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[serde_as]
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum MapOrVec {
            Vec(Vec<String>),
            Map(
                #[serde_as(as = "IndexMap<_, DisplayFromStr, FxBuildHasher>")]
                IndexMap<PackageName, NamelessMatchSpec, FxBuildHasher>,
            ),
        }

        Ok(match MapOrVec::deserialize(deserializer)? {
            MapOrVec::Vec(v) => v,
            MapOrVec::Map(m) => m
                .into_iter()
                .map(|(name, spec)| MatchSpec::from_nameless(spec, Some(name)).to_string())
                .collect(),
        })
    }
}

pub(crate) struct Pep440MapOrVec;

impl<'de> DeserializeAs<'de, Vec<String>> for Pep440MapOrVec {
    fn deserialize_as<D>(deserializer: D) -> Result<Vec<String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[serde_as]
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum MapOrVec {
            Vec(Vec<String>),
            Map(
                #[serde_as(as = "IndexMap<_, DisplayFromStr, FxBuildHasher>")]
                IndexMap<String, pep440_rs::VersionSpecifiers, FxBuildHasher>,
            ),
        }

        Ok(match MapOrVec::deserialize(deserializer)? {
            MapOrVec::Vec(v) => v,
            MapOrVec::Map(m) => m
                .into_iter()
                .map(|(name, spec)| {
                    pep508_rs::Requirement {
                        name,
                        extras: None,
                        version_or_url: if spec.is_empty() {
                            None
                        } else {
                            Some(VersionOrUrl::VersionSpecifier(spec))
                        },
                        marker: None,
                    }
                    .to_string()
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use serde::Deserialize;
    use serde_with::serde_as;

    #[test]
    fn test_parse_dependencies_as_map_or_vec() {
        #[serde_as]
        #[derive(Deserialize, Eq, PartialEq, Debug)]
        struct Data {
            #[serde_as(deserialize_as = "MatchSpecMapOrVec")]
            dependencies: Vec<String>,
        }

        let data_with_map: Data = serde_yaml::from_str("dependencies:\n  foo: \">3.12\"").unwrap();
        let data_with_vec: Data = serde_yaml::from_str("dependencies:\n- \"foo >3.12\"").unwrap();
        assert_eq!(data_with_map, data_with_vec);
    }
}
