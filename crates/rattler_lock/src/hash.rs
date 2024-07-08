use std::cmp::Ordering;

use rattler_digest::{serde::SerializableHash, Md5Hash, Sha256Hash};
use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use serde_with::skip_serializing_none;

/// This implementation of the `Deserialize` trait for the `PackageHashes` struct
///
/// It expects the input to have either a `md5` field, a `sha256` field, or both.
/// If both fields are present, it constructs a `Md5Sha256` instance with their values.
/// If only the `md5` field is present, it constructs a `Md5` instance with its value.
/// If only the `sha256` field is present, it constructs a `Sha256` instance with its value.
/// If neither field is present it returns an error
#[derive(Eq, PartialEq, Hash, Clone, Debug)]
pub enum PackageHashes {
    /// Contains an MD5 hash
    Md5(Md5Hash),
    /// Contains as Sha256 Hash
    Sha256(Sha256Hash),
    /// Contains both hashes
    Md5Sha256(Md5Hash, Sha256Hash),
}

impl PartialOrd for PackageHashes {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PackageHashes {
    fn cmp(&self, other: &Self) -> Ordering {
        self.to_vec().cmp(&other.to_vec())
    }
}

impl PackageHashes {
    /// Create correct enum from hashes
    pub fn from_hashes(md5: Option<Md5Hash>, sha256: Option<Sha256Hash>) -> Option<PackageHashes> {
        use PackageHashes::{Md5, Md5Sha256, Sha256};
        match (md5, sha256) {
            (Some(md5), None) => Some(Md5(md5)),
            (None, Some(sha256)) => Some(Sha256(sha256)),
            (Some(md5), Some(sha256)) => Some(Md5Sha256(md5, sha256)),
            (None, None) => None,
        }
    }

    /// Returns the Sha256 hash
    pub fn sha256(&self) -> Option<&Sha256Hash> {
        match self {
            PackageHashes::Md5(_) => None,
            PackageHashes::Sha256(sha256) | PackageHashes::Md5Sha256(_, sha256) => Some(sha256),
        }
    }

    /// Returns the MD5 hash
    pub fn md5(&self) -> Option<&Md5Hash> {
        match self {
            PackageHashes::Sha256(_) => None,
            PackageHashes::Md5(md5) | PackageHashes::Md5Sha256(md5, _) => Some(md5),
        }
    }

    /// Returns bit pattern
    fn to_vec(&self) -> Vec<u8> {
        match self {
            PackageHashes::Sha256(sha256) => sha256.to_vec(),
            PackageHashes::Md5(md5) => md5.to_vec(),
            PackageHashes::Md5Sha256(md5, sha256) => [md5.to_vec(), sha256.to_vec()].concat(),
        }
    }
}

#[skip_serializing_none]
#[derive(Serialize, Deserialize)]
struct RawPackageHashes {
    md5: Option<SerializableHash<rattler_digest::Md5>>,
    sha256: Option<SerializableHash<rattler_digest::Sha256>>,
}

impl Serialize for PackageHashes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use PackageHashes::{Md5, Md5Sha256, Sha256};
        let raw = match self {
            Md5(hash) => RawPackageHashes {
                md5: Some(SerializableHash::from(*hash)),
                sha256: None,
            },
            Sha256(hash) => RawPackageHashes {
                md5: None,
                sha256: Some(SerializableHash::from(*hash)),
            },
            Md5Sha256(md5hash, sha) => RawPackageHashes {
                md5: Some(SerializableHash::from(*md5hash)),
                sha256: Some(SerializableHash::from(*sha)),
            },
        };
        raw.serialize(serializer)
    }
}

// This implementation of the `Deserialize` trait for the `PackageHashes` struct
//
// It expects the input to have either a `md5` field, a `sha256` field, or both.
// If both fields are present, it constructs a `Md5Sha256` instance with their values.
// If only the `md5` field is present, it constructs a `Md5` instance with its value.
// If only the `sha256` field is present, it constructs a `Sha256` instance with its value.
// If neither field is present it returns an error
impl<'de> Deserialize<'de> for PackageHashes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use PackageHashes::{Md5, Md5Sha256, Sha256};
        let temp = RawPackageHashes::deserialize(deserializer)?;
        Ok(match (temp.md5, temp.sha256) {
            (Some(md5), Some(sha)) => Md5Sha256(md5.into(), sha.into()),
            (Some(md5), None) => Md5(md5.into()),
            (None, Some(sha)) => Sha256(sha.into()),
            _ => {
                return Err(D::Error::custom(
                    "Expected `sha256` field `md5` field or both",
                ))
            }
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use serde_yaml::from_str;

    #[test]
    fn test_package_hashes() {
        let yaml = r#"
          md5: 4eccaeba205f0aed9ac3a9ea58568ca3
          sha256: f240217476e148e825420c6bc3a0c0efb08c0718b7042fae960400c02af858a3
    "#;

        let result: PackageHashes = from_str(yaml).unwrap();
        assert!(matches!(result, PackageHashes::Md5Sha256(_, _)));

        let yaml = r#"
          md5: 4eccaeba205f0aed9ac3a9ea58568ca3
    "#;

        let result: PackageHashes = from_str(yaml).unwrap();
        assert!(matches!(result, PackageHashes::Md5(_)));

        let yaml = r#"
          sha256: f240217476e148e825420c6bc3a0c0efb08c0718b7042fae960400c02af858a3
    "#;

        let result: PackageHashes = from_str(yaml).unwrap();
        assert!(matches!(result, PackageHashes::Sha256(_)));
    }
}
