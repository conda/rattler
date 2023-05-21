//! # JLAP
//!
//! This module contains functions and data types for downloading and applying patches from JLAP
//! files.
//!
//! JLAP files provide a way to incrementally retrieve and build the `repodata.json` files
//! that conda compatible applications use to query conda packages. The first time you download
//! this file you will build the entire `repodata.json` from scratch, but subsequent request
//! can retrieve only the updates to the file, which can have a drastic effect on how fast
//! this file is updated.
//!
//!
use std::path::{Path, PathBuf};
use itertools::Itertools;
use reqwest::{Client, Response, header::{HeaderMap, HeaderValue}};
use serde::{Serialize, Deserialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

/// File suffix for JLAP files
pub const JLAP_FILE_SUFFIX: &str = "jlap";

/// File name of JLAP file
pub const JLAP_FILE_NAME: &str = "repodata.jlap";

/// File suffix for JLAP files
pub const JLAP_FOOTER_OFFSET: usize = 2;

/// Represents the variety of errors that we come across while processing JLAP files
#[derive(Debug, thiserror::Error)]
pub enum JLAPError {
    #[error(transparent)]
    /// Pass-thru for JSON errors found while parsing JLAP file
    JSONParseError(serde_json::Error),

    #[error("No patches found in the JLAP file")]
    /// Error returned when JLAP file has no patches in it
    NoPatchesFoundError,

    #[error("No matching hashes can be found in the JLAP file")]
    /// Error returned when none of the patches match the hash of our current `repodata.json`
    NoHashesFoundError
}

/// Represents the numerous patches found in a JLAP file which makes up a majority
/// of the file
#[derive(Serialize, Deserialize, Debug)]
pub struct Patch {
    /// Next hash of `repodata.json` file
    pub to: String,

    /// Previous hash of `repodata.json` file
    pub from: String,

    /// Patches to apply to current `repodata.json` file
    pub patch: json_patch::Patch, // [] is a valid, empty patch
}

/// Represents the metadata for a JLAP file, which is typically found at the very end
#[derive(Serialize, Deserialize, Debug)]
pub struct JLAPMetadata {
    url: String,
    latest: String
}

// #[derive(Debug)]
// pub struct JLAP {
//     patches: Vec<String>
// }

/// Fetches a JLAP object from server
///
pub async fn fetch_jlap (url: &str, client: &Client, range: Option<String>) -> Result<Response, reqwest::Error> {
    let request_builder = client.get(url);
    let mut headers = HeaderMap::default();

    if let Some(value) = range {
        headers.insert(
            reqwest::header::RANGE, HeaderValue::from_str(&value).unwrap()
        );
    }

    request_builder.headers(headers).send().await
}


/// Builds a cache key used in storing JLAP cache
pub fn get_jlap_cache_key(subdir_url: &Url, cache_path: &Path) -> String {
    let cache_key = crate::utils::url_to_cache_filename(&subdir_url);

    format!("{}.{}", cache_key, JLAP_FILE_SUFFIX)

}

/// Persist a JLAP file to a cache location
///
/// This is done by first determining where the file should be written to provided
/// the input arguments, writing the file and then return the `PathBuf` object so the
/// caller can also keep track of where this is stored.
pub async fn cache_jlap_response (subdir_url: &Url, response_bytes: &[u8], cache_path: &Path) -> Result<PathBuf, tokio::io::Error>{
    let jlap_cache_key = get_jlap_cache_key(subdir_url, cache_path);
    let jlap_cache_path = cache_path.join(jlap_cache_key);

    let mut jlap_file = tokio::fs::File::create(&jlap_cache_path).await?;
    jlap_file.write_all(response_bytes).await?;

    Ok(jlap_cache_path)
}

/// Determines if a JLAP cache file already exists on the filesystem
pub fn cache_jlap_exists(subdir_url: &Url, cache_path: &Path) -> bool {
    let jlap_cache_key = get_jlap_cache_key(subdir_url, cache_path);
    let jlap_cache_path = cache_path.join(jlap_cache_key);

    jlap_cache_path.is_file()
}

/// Determines the byte offset to use for JLAP range requests
///
/// This file assumes we already have a locally cached version of the JLAP file
pub async fn get_jlap_request_range(subdir_url: &Url, cache_path: &Path) -> Result<String, tokio::io::Error> {
    let cache_key = get_jlap_cache_key(subdir_url, cache_path);
    let jlap_cache_path = cache_path.join(cache_key);

    let mut cache_file = tokio::fs::File::open(jlap_cache_path).await?;
    let mut contents = String::from("");

    cache_file.read_to_string(&mut contents).await?;

    let lines: Vec<&str> = contents.split("\n").collect();
    let length = lines.len();

    if length > 1 {
        let patches = lines[0..length - JLAP_FOOTER_OFFSET].iter().join("\n");
        return Ok(format!("bytes={}-", patches.into_bytes().len()));
    }

    // We default to starting from the beginning of the file.
    Ok(String::from("bytes=0-"))
}

fn parse_patch_json(line: &&str) -> Result<Patch, JLAPError> {
    serde_json::from_str(
        line
    ).map_err(
        JLAPError::JSONParseError
    )
}

/// Converts the body of a JLAP request to JSON objects
///
/// We take the text value and the offset value. Sometimes this string
/// may be begin with a hash value that we will want to skip via the offset
/// value.
pub fn convert_jlap_string_to_patch_set (text: &str, offset: usize) -> Result<Vec<Patch>, JLAPError>  {
    let lines: Vec<&str> = text.split("\n").collect();
    let length = lines.len();

    if length > 1 {
        let patch_lines = lines[offset..length - JLAP_FOOTER_OFFSET].iter();
        let patches: Result<Vec<Patch>, JLAPError> = patch_lines.map(
            parse_patch_json
        ).collect();

        return match patches {
            Ok(patches) => {
                if patches.len() > 0 {
                    Ok(patches)
                } else {
                    Err(JLAPError::NoPatchesFoundError)
                }
            },
            Err(error) => {
                Err(error)
            }
        }
    }

    return Err(JLAPError::NoPatchesFoundError);
}

#[cfg(test)]
mod test {
    use super::{
        convert_jlap_string_to_patch_set, JLAPError
    };
    use assert_matches::assert_matches;

    const FAKE_JLAP_DATA: &str = r#"
ea3f3b1853071a4b1004b9f33594938b01e01cc8ca569f20897e793c35037de4
{"to": "20af8f45bf8bc15e404bea61f608881c2297bee8a8917bee1de046da985d6d89", "from": "4324630c4aa09af986e90a1c9b45556308a4ec8a46cee186dd7013cdd7a251b7", "patch": [{"op": "add", "path": "/packages/snowflake-snowpark-python-0.10.0-py38hecd8cb5_0.tar.bz2", "value": {"build": "py38hecd8cb5_0", "build_number": 0, "constrains": ["pandas >1,<1.4"], "depends": ["cloudpickle >=1.6.0,<=2.0.0", "python >=3.8,<3.9.0a0", "snowflake-connector-python >=2.7.12", "typing-extensions >=4.1.0"], "license": "Apache-2.0", "license_family": "Apache", "md5": "91fc7aac6ea0c4380a334b77455b1454", "name": "snowflake-snowpark-python", "sha256": "3cbfed969c8702673d1b281e8dd7122e2150d27f8963d1d562cd66b3308b0b31", "size": 359503, "subdir": "osx-64", "timestamp": 1663585464882, "version": "0.10.0"}}, {"op": "add", "path": "/packages.conda/snowflake-snowpark-python-0.10.0-py38hecd8cb5_0.conda", "value": {"build": "py38hecd8cb5_0", "build_number": 0, "constrains": ["pandas >1,<1.4"], "depends": ["cloudpickle >=1.6.0,<=2.0.0", "python >=3.8,<3.9.0a0", "snowflake-connector-python >=2.7.12", "typing-extensions >=4.1.0"], "license": "Apache-2.0", "license_family": "Apache", "md5": "7353a428613fa62f4c8ec9b5a1e4f16d", "name": "snowflake-snowpark-python", "sha256": "e3b5fa220262e23480d32a883b19971d1bd88df33eb90e9556e2a3cfce32b0a4", "size": 316623, "subdir": "osx-64", "timestamp": 1663585464882, "version": "0.10.0"}}]}
{"url": "repodata.json", "latest": "20af8f45bf8bc15e404bea61f608881c2297bee8a8917bee1de046da985d6d89"}
c540a2ab0ab4674dada39063205a109d26027a55bd8d7a5a5b711be03ffc3a9d"#;

    #[test]
    pub fn test_convert_jlap_string_to_patch_set_no_patches_found() {
        // TODO: this would be way better as a parameterized test.
        let test_string = "bad_data\nbad_data\nbad_data\nbad_data";

        assert_matches!(
            convert_jlap_string_to_patch_set(test_string, 1),
            Err(JLAPError::NoPatchesFoundError)
        );

        let test_string = "bad_data\nbad_data";

        assert_matches!(
            convert_jlap_string_to_patch_set(test_string, 1),
            Err(JLAPError::NoPatchesFoundError)
        );

        let test_string = "";

        assert_matches!(
            convert_jlap_string_to_patch_set(test_string, 1),
            Err(JLAPError::NoPatchesFoundError)
        );
    }

    #[test]
    pub fn test_convert_jlap_string_to_patch_set_success() {
        let patches = convert_jlap_string_to_patch_set(FAKE_JLAP_DATA).unwrap();

        assert_eq!(patches.len(), 1);
    }
}