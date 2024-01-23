use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::__private__::{DeserializeAsWrap, SerializeAsWrap};
use serde_with::{DeserializeAs, SerializeAs};
use std::collections::HashSet;
use std::hash::{BuildHasher, Hash};
use std::marker::PhantomData;

/// Used with `serde_with` to serialize a collection as a sorted collection.
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
        Ok(content.into_iter().collect())
    }
}

impl<T: Ord, HS, TAs: SerializeAs<T>> SerializeAs<HashSet<T, HS>> for Ordered<TAs> {
    fn serialize_as<S>(source: &HashSet<T, HS>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut elements: Vec<_> = source.iter().collect();
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
        let mut elements: Vec<_> = source.iter().collect();
        elements.sort();
        SerializeAsWrap::<Vec<&T>, Vec<&TAs>>::new(&elements).serialize(serializer)
    }
}
