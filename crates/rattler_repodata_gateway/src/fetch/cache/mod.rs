mod cache_headers;

pub use cache_headers::CacheHeaders;
use fs_err as fs;
use rattler_digest::{serde::SerializableHash, Blake2b256};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::serde_as;
use std::{path::Path, str::FromStr, time::SystemTime};
use url::Url;

/// Representation of the `.info.json` file alongside a `repodata.json` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoDataState {
    /// The URL from where the repodata was downloaded. This is the URL of the `repodata.json`,
    /// `repodata.json.zst`, or another variant. This is different from the subdir url which does
    /// NOT include the final filename.
    pub url: Url,

    /// The HTTP cache headers send along with the last response.
    #[serde(flatten)]
    pub cache_headers: CacheHeaders,

    /// The timestamp of the repodata.json on disk
    #[serde(
        deserialize_with = "duration_from_nanos",
        serialize_with = "duration_to_nanos",
        rename = "mtime_ns"
    )]
    pub cache_last_modified: SystemTime,

    /// The size of the repodata.json file on disk.
    #[serde(rename = "size")]
    pub cache_size: u64,

    /// The blake2 hash of the file
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_blake2_hash",
        serialize_with = "serialize_blake2_hash"
    )]
    pub blake2_hash: Option<blake2::digest::Output<Blake2b256>>,

    /// Upstream hash represented by the on-disk file. Used for jlap which reformats the cached json
    /// but knows equivalent remote repodata.json hashes.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_blake2_hash",
        serialize_with = "serialize_blake2_hash"
    )]
    pub blake2_hash_nominal: Option<blake2::digest::Output<Blake2b256>>,

    /// Whether or not zst is available for the subdirectory
    pub has_zst: Option<Expiring<bool>>,

    /// Whether a bz2 compressed version is available for the subdirectory
    pub has_bz2: Option<Expiring<bool>>,

    /// Whether or not JLAP is available for the subdirectory
    pub has_jlap: Option<Expiring<bool>>,

    /// State information related to JLAP
    pub jlap: Option<JLAPState>,
}

impl RepoDataState {
    /// Reads and parses a file from disk.
    pub fn from_path(path: &Path) -> Result<RepoDataState, std::io::Error> {
        let content = fs::read_to_string(path)?;
        Ok(Self::from_str(&content)?)
    }

    /// Save the cache state to the specified file.
    pub fn to_path(&self, path: &Path) -> Result<(), std::io::Error> {
        let file = fs::File::create(path)?;
        Ok(serde_json::to_writer_pretty(file, self)?)
    }
}

impl FromStr for RepoDataState {
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
    }
}

/// Used inside of the `RepoDataState` to store information related to our JLAP state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JLAPState {
    /// Initialization Vector (IV) for of the JLAP file; this is found on the first line of the
    /// JLAP file.
    #[serde(rename = "iv", with = "hex")]
    pub initialization_vector: Vec<u8>,

    /// Current position to use for the bytes offset in the range request for JLAP
    #[serde(rename = "pos")]
    pub position: u64,

    /// Footer contains metadata about the JLAP file such as which url it is for
    pub footer: JLAPFooter,
}

/// Represents the metadata for a JLAP file, which is typically found at the very end
#[serde_as]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JLAPFooter {
    /// This is not actually a full URL, just the last part of it (i.e. the filename
    /// `repodata.json`). That's why we store it as a [`String`]
    pub url: String,

    /// blake2b hash of the latest `repodata.json` file
    #[serde_as(as = "SerializableHash::<rattler_digest::Blake2b256>")]
    pub latest: blake2::digest::Output<Blake2b256>,
}

/// Represents a value and when the value was last checked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Expiring<T> {
    pub value: T,

    // #[serde(with = "chrono::serde::ts_seconds")]
    pub last_checked: chrono::DateTime<chrono::Utc>,
}

impl<T> Expiring<T> {
    pub fn value(&self, expiration: chrono::Duration) -> Option<&T> {
        if chrono::Utc::now().signed_duration_since(self.last_checked) >= expiration {
            None
        } else {
            Some(&self.value)
        }
    }
}

/// Deserializes a [`SystemTime`] by parsing an integer and converting that as a nanosecond based unix
/// epoch timestamp to a [`SystemTime`].
fn duration_from_nanos<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;
    SystemTime::UNIX_EPOCH
        .checked_add(std::time::Duration::from_nanos(Deserialize::deserialize(
            deserializer,
        )?))
        .ok_or_else(|| D::Error::custom("the time cannot be represented internally"))
}

/// Serializes a [`SystemTime`] by converting it to a nanosecond based unix epoch timestamp.
fn duration_to_nanos<S: Serializer>(time: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
    use serde::ser::Error;
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_err| S::Error::custom("duration cannot be computed for file time"))?
        .as_nanos()
        .serialize(s)
}

fn deserialize_blake2_hash<'de, D>(
    deserializer: D,
) -> Result<Option<blake2::digest::Output<Blake2b256>>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;
    match Option::<&'de str>::deserialize(deserializer)? {
        Some(str) => Ok(Some(
            rattler_digest::parse_digest_from_hex::<Blake2b256>(str)
                .ok_or_else(|| D::Error::custom("failed to parse blake2 hash"))?,
        )),
        None => Ok(None),
    }
}

fn serialize_blake2_hash<S: Serializer>(
    time: &Option<blake2::digest::Output<Blake2b256>>,
    s: S,
) -> Result<S::Ok, S::Error> {
    match time.as_ref() {
        None => s.serialize_none(),
        Some(hash) => format!("{hash:x}").serialize(s),
    }
}

#[cfg(test)]
mod test {
    use super::RepoDataState;
    use std::str::FromStr;

    const JSON_STATE_ONE: &str = r#"{
        "cache_control": "public, max-age=1200",
        "etag": "\"bec332621e00fc4ad87ba185171bcf46\"",
        "has_zst": {
            "last_checked": "2023-02-13T14:08:50Z",
            "value": true
        },
        "mod": "Mon, 13 Feb 2023 13:49:56 GMT",
        "mtime_ns": 1676297333020928000,
        "size": 156627374,
        "url": "https://conda.anaconda.org/conda-forge/win-64/repodata.json.zst"
    }"#;

    const JSON_STATE_TWO: &str = r#"{
      "url": "https://repo.anaconda.com/pkgs/main/osx-64/repodata.json.zst",
      "etag": "W/\"2f8b1ff101d75e40adf28c3fcbcd330b\"",
      "mod": "Thu, 18 May 2023 13:28:44 GMT",
      "cache_control": "public, max-age=30",
      "mtime_ns": 1684418349941482000,
      "size": 38001429,
      "blake2_hash": "a1bb42ccd11d5610189380b8b0a71ca0fa7e3273ff6235ae1d543606041eb3bd",
      "has_zst": {
        "value": true,
        "last_checked": "2023-05-18T13:59:07.112638Z"
      },
      "has_bz2": {
        "value": true,
        "last_checked": "2023-05-18T13:59:07.112638Z"
      },
      "has_jlap": null
    }"#;

    #[test]
    pub fn test_parse_repo_data_state_one() {
        insta::assert_yaml_snapshot!(RepoDataState::from_str(JSON_STATE_ONE).unwrap());
    }

    #[test]
    pub fn test_parse_repo_data_state_two() {
        insta::assert_yaml_snapshot!(RepoDataState::from_str(JSON_STATE_TWO).unwrap());
    }
}
