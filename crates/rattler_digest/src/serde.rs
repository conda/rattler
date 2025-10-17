//! Provides custom serialization/deserialization functions for [`Output`] of a [`Digest`].
//!
//! Use the struct [`SerializableHash`] to easily serialize digest outputs. This type
//! automatically handles both human-readable formats (like JSON) and binary formats
//! (like MessagePack). See [`SerializableHash`] for detailed documentation on the
//! serialization behavior.

#![allow(clippy::doc_markdown)]

use digest::{Digest, Output, OutputSizeUser};
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DeserializeAs, SerializeAs};
use std::borrow::Cow;
use std::fmt::LowerHex;
use std::ops::Deref;

/// Deserialize the [`Output`] of a [`Digest`].
///
/// If the deserializer is human-readable, it will parse the digest from a hex
/// string. Otherwise, it will deserialize raw bytes.
pub fn deserialize<'de, D, Dig: Digest>(deserializer: D) -> Result<Output<Dig>, D::Error>
where
    D: Deserializer<'de>,
{
    if deserializer.is_human_readable() {
        let str = Cow::<'de, str>::deserialize(deserializer)?;
        super::parse_digest_from_hex::<Dig>(str.as_ref())
            .ok_or_else(|| Error::custom("failed to parse digest"))
    } else {
        let bytes = Cow::<'de, [u8]>::deserialize(deserializer)?;
        let len = <Dig as OutputSizeUser>::output_size();
        if bytes.len() != len {
            return Err(Error::custom(format!(
                "invalid length, expected {}, got {}",
                len,
                bytes.len()
            )));
        }
        Ok(Output::<Dig>::clone_from_slice(&bytes))
    }
}

/// Serializes the [`Output`] of a [`Digest`].
///
/// If the serializer is human-readable, it will write the digest as a hex
/// string. Otherwise, it will deserialize raw bytes.
pub fn serialize<'a, S: Serializer, Dig: Digest>(
    digest: &'a Output<Dig>,
    s: S,
) -> Result<S::Ok, S::Error>
where
    &'a Output<Dig>: LowerHex,
{
    if s.is_human_readable() {
        format!("{digest:x}").serialize(s)
    } else {
        // Without specialization, Rust forces Serde to treat &[u8] just like any other slice and
        // Vec<u8> just like any other vector. In reality this particular slice and vector can
        // often be serialized and deserialized in a more efficient, compact representation in many
        // formats.
        //
        // Using the `serde_bytes` crate, we can opt into specialized handling.
        serde_bytes::Bytes::new(digest.as_ref()).serialize(s)
    }
}

/// Wrapper type for easily serializing and deserializing cryptographic hash digests.
///
/// This type provides automatic format-aware serialization:
/// - **Human-readable formats** (JSON, YAML, TOML, etc.): Serializes as a lowercase hexadecimal string
/// - **Binary formats** (MessagePack, Bincode, etc.): Serializes as raw bytes using `serde_bytes`
///
/// # Deserialization Behavior
///
/// ## Human-Readable Formats
/// Accepts a hexadecimal string (case-insensitive) and parses it into the digest output.
/// Returns an error if the string is not valid hexadecimal or has incorrect length.
///
/// ## Binary Formats
/// Accepts raw bytes in multiple MessagePack encoding formats:
/// - **Binary format** (`bin 8/16/32`): Most efficient, preferred format
/// - **Array format** (`fixarray/array 16/32`): Array of integers, for compatibility
///
/// Returns an error if the byte length doesn't match the expected digest size.
///
/// # Examples
///
/// ## JSON (Human-Readable)
/// ```
/// use rattler_digest::serde::SerializableHash;
///
/// let hash = SerializableHash::<sha2::Sha256>(
///     rattler_digest::parse_digest_from_hex::<sha2::Sha256>(
///         "fe51de6107f9edc7aa4f786a70f4a883943bc9d39b3bb7307c04c41410990726"
///     ).unwrap()
/// );
///
/// // Serializes as: "fe51de6107f9edc7aa4f786a70f4a883943bc9d39b3bb7307c04c41410990726"
/// let json = serde_json::to_string(&hash).unwrap();
/// let deserialized: SerializableHash<sha2::Sha256> = serde_json::from_str(&json).unwrap();
/// ```
///
/// ## MessagePack (Binary)
/// ```
/// use rattler_digest::serde::SerializableHash;
///
/// let hash = SerializableHash::<sha2::Sha256>(
///     rattler_digest::parse_digest_from_hex::<sha2::Sha256>(
///         "fe51de6107f9edc7aa4f786a70f4a883943bc9d39b3bb7307c04c41410990726"
///     ).unwrap()
/// );
///
/// // Serializes as raw 32 bytes using MessagePack binary format
/// let bytes = rmp_serde::to_vec(&hash).unwrap();
/// let deserialized: SerializableHash<sha2::Sha256> = rmp_serde::from_slice(&bytes).unwrap();
/// ```
#[derive(Debug, Clone)]
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

    #[test]
    pub fn test_serializable_hash_messagepack() {
        let hash = SerializableHash::<sha2::Sha256>(
            crate::parse_digest_from_hex::<sha2::Sha256>(
                "fe51de6107f9edc7aa4f786a70f4a883943bc9d39b3bb7307c04c41410990726",
            )
            .unwrap(),
        );

        // Serialize to MessagePack (binary format, non-human-readable)
        let bytes = rmp_serde::to_vec(&hash).unwrap();

        // Deserialize from MessagePack
        let deserialized: SerializableHash<sha2::Sha256> = rmp_serde::from_slice(&bytes).unwrap();

        // Verify the hash matches
        assert_eq!(&hash.0, &deserialized.0);
    }

    #[test]
    pub fn test_deserialize_messagepack_raw_bytes() {
        use rmp::encode;

        let hex_str = "fe51de6107f9edc7aa4f786a70f4a883943bc9d39b3bb7307c04c41410990726";
        let expected_hash = SerializableHash::<sha2::Sha256>(
            crate::parse_digest_from_hex::<sha2::Sha256>(hex_str).unwrap(),
        );

        // Get the raw bytes from the expected hash
        let hash_bytes = expected_hash.0.as_slice();

        // Test 1: MessagePack binary format (bin 8, bin 16, or bin 32)
        let mut msgpack_bin = Vec::new();
        encode::write_bin(&mut msgpack_bin, hash_bytes).unwrap();

        let deserialized_bin: SerializableHash<sha2::Sha256> =
            rmp_serde::from_slice(&msgpack_bin).unwrap();
        assert_eq!(&expected_hash.0, &deserialized_bin.0);

        // Test 2: MessagePack array format (fixarray or array 16/32)
        let mut msgpack_array = Vec::new();
        encode::write_array_len(&mut msgpack_array, hash_bytes.len() as u32).unwrap();
        for byte in hash_bytes {
            encode::write_uint(&mut msgpack_array, u64::from(*byte)).unwrap();
        }

        let deserialized_array: SerializableHash<sha2::Sha256> =
            rmp_serde::from_slice(&msgpack_array).unwrap();
        assert_eq!(&expected_hash.0, &deserialized_array.0);
    }

    #[test]
    pub fn test_deserialize_messagepack_invalid_length() {
        use rmp::encode;

        // Test 1: Binary format with incorrect length (too short)
        let wrong_length_bytes = [0u8; 16]; // Only 16 bytes instead of 32
        let mut msgpack_bin_short = Vec::new();
        encode::write_bin(&mut msgpack_bin_short, &wrong_length_bytes).unwrap();

        let result: Result<SerializableHash<sha2::Sha256>, _> =
            rmp_serde::from_slice(&msgpack_bin_short);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid length") || err.contains("expected 32"));

        // Test 2: Binary format with incorrect length (too long)
        let wrong_length_bytes = [0u8; 64]; // 64 bytes instead of 32
        let mut msgpack_bin_long = Vec::new();
        encode::write_bin(&mut msgpack_bin_long, &wrong_length_bytes).unwrap();

        let result: Result<SerializableHash<sha2::Sha256>, _> =
            rmp_serde::from_slice(&msgpack_bin_long);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid length") || err.contains("expected 32"));

        // Test 3: Array format with incorrect length (too short)
        let mut msgpack_array_short = Vec::new();
        encode::write_array_len(&mut msgpack_array_short, 16).unwrap();
        for _ in 0..16 {
            encode::write_uint(&mut msgpack_array_short, 0).unwrap();
        }

        let result: Result<SerializableHash<sha2::Sha256>, _> =
            rmp_serde::from_slice(&msgpack_array_short);
        assert!(result.is_err());

        // Test 4: Array format with incorrect length (too long)
        let mut msgpack_array_long = Vec::new();
        encode::write_array_len(&mut msgpack_array_long, 64).unwrap();
        for _ in 0..64 {
            encode::write_uint(&mut msgpack_array_long, 0).unwrap();
        }

        let result: Result<SerializableHash<sha2::Sha256>, _> =
            rmp_serde::from_slice(&msgpack_array_long);
        assert!(result.is_err());
    }

    #[test]
    pub fn test_messagepack_serialized_size() {
        let hex_str = "fe51de6107f9edc7aa4f786a70f4a883943bc9d39b3bb7307c04c41410990726";
        let hash = SerializableHash::<sha2::Sha256>(
            crate::parse_digest_from_hex::<sha2::Sha256>(hex_str).unwrap(),
        );

        // Serialize to MessagePack
        let bytes = rmp_serde::to_vec(&hash).unwrap();

        // SHA256 is 32 bytes, MessagePack bin8 format uses:
        // - 1 byte for the format marker (0xc4 for bin8)
        // - 1 byte for the length (32)
        // - 32 bytes for the data
        // Total: 34 bytes
        assert_eq!(bytes.len(), 34);

        // Verify the format is bin8
        assert_eq!(bytes[0], 0xc4); // bin8 marker
        assert_eq!(bytes[1], 32); // length
    }
}
