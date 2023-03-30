//! Provides custom serialization/deserialization functions for [`Output`] of a [`Digest`]
//! Use the struct [`SerializableHash`] to easily serialize the digest.
//!
//! # Example:
//!
//! ```
//!
//! use rattler_digest::serde::SerializableHash;
//!
//! let hash = SerializableHash::<sha2::Sha256>(
//! rattler_digest::parse_digest_from_hex::<sha2::Sha256>("fe51de6107f9edc7aa4f786a70f4a883943bc9d39b3bb7307c04c41410990726").unwrap());
//! let str = serde_json::to_string(&hash).unwrap();
//! let hash: SerializableHash<sha2::Sha256> = serde_json::from_str(&str).unwrap();
//!
//! ```
//!
use digest::{Digest, Output};
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DeserializeAs, SerializeAs};
use std::fmt::LowerHex;
use std::ops::Deref;

/// Deserialize into [`Output`] of a [`Digest`]
pub fn deserialize<'de, D, Dig: Digest>(deserializer: D) -> Result<Output<Dig>, D::Error>
where
    D: Deserializer<'de>,
{
    let str = <&'de str>::deserialize(deserializer)?;
    super::parse_digest_from_hex::<Dig>(str).ok_or_else(|| Error::custom("failed to parse digest"))
}

/// Serialize into a string
pub fn serialize<'a, S: Serializer, Dig: Digest>(
    digest: &'a Output<Dig>,
    s: S,
) -> Result<S::Ok, S::Error>
where
    &'a Output<Dig>: LowerHex,
{
    format!("{digest:x}").serialize(s)
}

/// Wrapper type for easily serializing a Hash
pub struct SerializableHash<T: Digest>(pub Output<T>);

impl<T: Digest> Serialize for SerializableHash<T>
where
    Output<T>: LowerHex,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize::<S, T>(&self.0, serializer)
    }
}

impl<'de, T: Digest + Default> Deserialize<'de> for SerializableHash<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let hash_output: Output<T> = deserialize::<D, T>(deserializer)?;
        Ok(SerializableHash(hash_output))
    }
}

impl<T: Digest> From<Output<T>> for SerializableHash<T> {
    fn from(output: Output<T>) -> Self {
        SerializableHash(output)
    }
}

impl<T: Digest> From<SerializableHash<T>> for Output<T> {
    fn from(s: SerializableHash<T>) -> Self {
        s.0
    }
}

impl<T: Digest> Deref for SerializableHash<T> {
    type Target = Output<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: Digest> SerializeAs<Output<T>> for SerializableHash<T>
where
    for<'a> &'a Output<T>: LowerHex,
{
    fn serialize_as<S>(source: &Output<T>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize::<S, T>(source, serializer)
    }
}

impl<'de, T: Digest + Default> DeserializeAs<'de, Output<T>> for SerializableHash<T> {
    fn deserialize_as<D>(deserializer: D) -> Result<Output<T>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize::<D, T>(deserializer)
    }
}

#[cfg(test)]
mod test {
    use crate::serde::SerializableHash;

    #[test]
    pub fn test_serializable_hash() {
        let hash = SerializableHash::<sha2::Sha256>(
            crate::parse_digest_from_hex::<sha2::Sha256>(
                "fe51de6107f9edc7aa4f786a70f4a883943bc9d39b3bb7307c04c41410990726",
            )
            .unwrap(),
        );
        let str = serde_json::to_string(&hash).unwrap();
        let _hash: SerializableHash<sha2::Sha256> = serde_json::from_str(&str).unwrap();
    }
}
