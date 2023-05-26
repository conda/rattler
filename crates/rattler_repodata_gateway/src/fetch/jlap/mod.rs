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
use std::str;
use itertools::Itertools;
use reqwest::{Client, Response, header::{HeaderMap, HeaderValue}};
use serde::{Serialize, Deserialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

use crate::fetch::CachedRepoData;

/// File suffix for JLAP file
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

    #[error(transparent)]
    /// Pass-thru for HTTP errors encountered while requesting JLAP
    HTTPError(reqwest::Error),

    #[error(transparent)]
    /// Pass-thru for file system errors encountered while requesting JLAP
    FileSystemError(tokio::io::Error),

    #[error("No patches found in the JLAP file")]
    /// Error returned when JLAP file has no patches in it
    NoPatchesFoundError,

    #[error("No matching hashes can be found in the JLAP file")]
    /// Error returned when none of the patches match the hash of our current `repodata.json`
    NoHashesFoundError,
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
    /// URL of the `repodata.json` file
    pub url: String,

    /// blake2b hash of the latest `repodata.json` file
    pub latest: String
}

/// Encapsulates data and behavior related to patching `repodata.json` with remote
/// `repodata.jlap` data.
#[derive(Debug)]
pub struct JLAPManager<'a, 'b> {
    /// Subdir URL; this is used to construct the URL for fetching JLAP data
    subdir_url: Url,

    /// HTTP client used to make requests
    client: &'a Client,

    /// Path to local cache folder; this is where our methods read/write JLAP cache
    cache_path: &'b Path,

    /// Hash of the current `repodata.json` file
    blake2_hash: Option<String>,

    /// Path to the current cached copy of `repodata.jlap`
    pub repo_data_jlap_path: PathBuf,

    /// Range request data
    pub range: Option<String>,

    /// Remote URL where JLAP data can be fetched
    pub jlap_url: Url,

    /// Offset to use when reading JLAP response; 0 means partial response; 1 means full response
    pub offset: usize,
}

impl<'a, 'b> JLAPManager<'a, 'b> {
    /// Creates a new JLAP object
    ///
    /// This associated function is a special constructor method for the JLAP struct.
    /// It is used to check for the existence of a cached copy of the JLAP file and to
    /// store some of what we need to fetch new JLAP data.
    pub async fn new(
        subdir_url: Url,
        client: &'a Client,
        cache_path: &'b Path,
        blake2_hash: Option<String>
    ) -> JLAPManager<'a, 'b> {
        // Determines the range for our request; error are okay, we fallback to `None`
        let repo_data_jlap_path= get_jlap_cache_path(&subdir_url, &cache_path);

        // This is the byte offset range we use while fetching JLAP updates
        let range = if repo_data_jlap_path.is_file() {
            match get_jlap_request_range(&repo_data_jlap_path).await {
                Ok(value) => if value == "" { None } else { Some(value) }
                // TODO: Maybe add a warning here? This means there was a problem opening
                //       and reading the file.
                Err(_) => None
            }
        } else {
            None
        };

        let jlap_url = subdir_url.join(JLAP_FILE_NAME).unwrap();

        let mut offset;
        if range == None {
            offset = 1;
        } else {
            offset = 0;
        }

        Self {
            subdir_url,
            client,
            cache_path,
            blake2_hash,
            repo_data_jlap_path,
            range,
            jlap_url,
            offset,
        }
    }

    /// Attempts to patch current `repodata.json` file
    ///
    /// This method first makes a request to fetch JLAP data given everything stored on the
    /// struct. If it successfully retrieves, it will then try to cache this file. This will
    /// either write a new file update the existing one with the new lines we fetched (if any).
    ///
    /// After this, it will then actually proceed to applying JSON patches to the `repo_data_json_path`
    /// file provided as an argument.
    pub async fn patch_repo_data(self, repo_data_json_path: &PathBuf) -> Result<(), JLAPError> {
        // Collect the JLAP file
        let result = fetch_jlap(
            self.jlap_url.as_str(),
            &self.client,
            self.range.clone()
        ).await;
        let response: Response = match result {
            Ok(response) => {
                response
            },
            Err(error) => {
                return Err(JLAPError::HTTPError(error));
            }
        };

        let response_text = response.text().await.unwrap();

        // Updates existing or creates new JLAP cache file
        self.save_jlap_cache(&response_text).await?;

        // At this point, our JLAP file is supposed to be up-to-date.
        // We can now read from it and find the applicable patches to
        // use with self.blake2_hash

        // TODO: Apply patches to `repo_data_json_path`
        Ok(())
    }

    /// Updates or creates the JLAP file we currently have cached
    ///
    /// If the file exists, then we update it otherwise, we just write an
    /// entire new file to cache.
    async fn save_jlap_cache(self, response_text: &str) -> Result<(), JLAPError> {
        if self.repo_data_jlap_path.is_file() {
            update_jlap_file(&self.repo_data_jlap_path, &response_text).await?;
            return Ok(())
        }

        match cache_jlap_response(&self.repo_data_jlap_path, &response_text).await {
            Ok(_) => {
                return Ok(())
            },
            Err(_) => {}  // TODO: this means we failed to write a cache file; maybe just log a warning?
        }

        Ok(())
    }
}


/// Fetches a JLAP response from server
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
pub fn get_jlap_cache_path(subdir_url: &Url, cache_path: &Path) -> PathBuf {
    let cache_key = crate::utils::url_to_cache_filename(&subdir_url);
    let cache_file_name = format!("{}.{}", cache_key, JLAP_FILE_SUFFIX);

    cache_path.join(cache_file_name)
}

/// Persist a JLAP file to the provided location
pub async fn cache_jlap_response (jlap_cache_path: &PathBuf, response_text: &str) -> Result<(), tokio::io::Error> {
    let response_bytes = response_text.as_bytes();
    let mut jlap_file = tokio::fs::File::create(&jlap_cache_path).await?;
    jlap_file.write_all(response_bytes).await?;

    Ok(())
}

/// Update an existing cached JLAP file
pub async fn update_jlap_file(jlap_file: &PathBuf, jlap_string: &str) -> Result<(), JLAPError> {
    let mut parts: Vec<&str> = jlap_string.split("\n").into_iter().filter(
        |s| !s.is_empty()
    ).collect();

    // We only care about updating if the response is greater than 2 lines.
    // This means we received some new patches.
    if parts.len() > 2 {
        let mut cache_file = match tokio::fs::File::open(jlap_file).await {
            Ok(value) => value,
            Err(err) => { return Err(JLAPError::FileSystemError(err)) }
        };

        let mut contents = String::new();
        match cache_file.read_to_string(&mut contents).await {
            Ok(_) => (),
            Err(error) => { return Err(JLAPError::FileSystemError(error)) }
        }

        let mut current_parts: Vec<&str> = contents.split("\n").collect();
        current_parts.truncate(current_parts.len() - 2);
        current_parts.extend(parts);

        let updated_jlap = current_parts.join("\n").into_bytes();

        let mut updated_file = match tokio::fs::File::create(jlap_file).await {
            Ok(file) => file,
            Err(err) => { return Err(JLAPError::FileSystemError(err)) }
        };

        return match updated_file.write_all(&updated_jlap).await {
            Ok(_) => Ok(()),
            Err(err) => Err(JLAPError::FileSystemError(err))
        };
    }

    Ok(())
}

/// Determines the byte offset to use for JLAP range requests
///
/// This function assumes we already have a locally cached version of the JLAP file
pub async fn get_jlap_request_range(jlap_cache_path: &PathBuf) -> Result<String, tokio::io::Error> {
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
    Ok(String::from(""))
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
pub fn convert_jlap_string_to_patch_set (
    text: &str,
    offset: usize
) -> Result<Vec<Patch>, JLAPError> {
    let lines: Vec<&str> = text.split("\n").filter(|&x| !x.is_empty()).collect();
    let length = lines.len();

    if length > 2 {
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