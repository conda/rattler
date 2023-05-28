//! # JLAP
//!
//! This module contains functions and data types for downloading and applying patches from JLAP
//! files.
//!
//! JLAP files provide a way to incrementally retrieve and build the `repodata.json` files
//! that conda compatible applications use to query conda packages. For more information about
//! how this file format works, please read this CEP proposal:
//!
//! - https://github.com/conda-incubator/ceps/pull/20/files
//!
//! ## Example
//!
//! The recommended way to use this module is by using the JLAPManager struct. This struct is meant
//! to act as a kind of "facade" object which orchestrates the underlying operations necessary
//! to fetch JLAP data used to update our current `repodata.json` file.
//!
//! Below is an example of how to initialize the struct and patch an existing `repodata.json` file:
//!
//! ```no_run
//! use std::{path::Path};
//! use reqwest::Client;
//! use url::Url;
//! use rattler_repodata_gateway::fetch::jlap::JLAPManager;
//!
//! #[tokio::main]
//! pub async fn main() {
//!     let repodata_url = Url::parse("https://conda.anaconda.org/conda-forge/osx-64/").unwrap();
//!     let client = Client::new();
//!     let cache = Path::new("./cache");
//!     let current_repo_data = cache.join("c93ef9c9.json");
//!     let current_repodata_hash = String::from(
//!         "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9"
//!     );
//!
//!     let manager = JLAPManager::new(
//!         repodata_url,
//!         &client,
//!         cache,
//!         Some(current_repodata_hash)
//!     )
//!     .await;
//!
//!     manager
//!         .patch_repo_data(&current_repo_data.clone())
//!         .await
//!         .unwrap();
//! }
//! ```
//!
//! ## TODO
//!
//! The following items still need to be implemented before this module should be considered
//! complete:
//!  - Use the checksum to validate our JLAP file after we update it
//!  - Generate a new `blake2b` hash of the updated repodata.json file and compare it to the
//!    metadata contained on the updated JLAP response. This also need to be made available
//!    so that we can update the `*.state.json` file
use reqwest::{
    header::{HeaderMap, HeaderValue},
    Client, Response,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::str;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

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
    /// Pass-thru for JSON errors found while patching
    JSONPatchError(json_patch::PatchError),

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
    NoHashFoundError,
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
    pub latest: String,
}

/// Encapsulates data and behavior related to patching `repodata.json` with remote
/// `repodata.jlap` data.
#[derive(Debug)]
pub struct JLAPManager<'a> {
    /// HTTP client used to make requests
    client: &'a Client,

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

impl<'a, 'b> JLAPManager<'a> {
    /// Creates a new JLAP object
    ///
    /// This associated function is a special constructor method for the JLAP struct.
    /// It is used to check for the existence of a cached copy of the JLAP file and to
    /// store some of what we need to fetch new JLAP data.
    pub async fn new(
        subdir_url: Url,
        client: &'a Client,
        cache_path: &Path,
        blake2_hash: Option<String>,
    ) -> JLAPManager<'a> {
        // Determines the range for our request; error are okay, we fallback to `None`
        let repo_data_jlap_path = get_jlap_cache_path(&subdir_url, cache_path);

        // This is the byte offset range we use while fetching JLAP updates
        let range = if repo_data_jlap_path.is_file() {
            match get_jlap_request_range(&repo_data_jlap_path).await {
                Ok(value) => value,
                // TODO: Maybe add a warning here? This means there was a problem opening
                //       and reading the file.
                Err(_) => None,
            }
        } else {
            None
        };

        let jlap_url = subdir_url.join(JLAP_FILE_NAME).unwrap();
        let offset = usize::from(range.is_none());

        Self {
            client,
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
        let result = fetch_jlap(self.jlap_url.as_str(), self.client, self.range.clone()).await;
        let response: Response = match result {
            Ok(response) => response,
            Err(error) => {
                return Err(JLAPError::HTTPError(error));
            }
        };

        let response_text = response.text().await.unwrap();

        // Updates existing or creates new JLAP cache file
        self.save_jlap_cache(&response_text).await?;

        let (metadata, patches) = get_jlap_metadata_and_patches(&self.repo_data_jlap_path).await?;

        // We already have the latest version; nothing to do
        let hash = self.blake2_hash.unwrap_or_default();
        if metadata.latest == hash {
            return Ok(());
        }

        let current_idx = find_current_patch_index(&patches, &hash);

        if let Some(idx) = current_idx {
            let applicable_patches: Vec<&Patch> = patches[idx..patches.len()].iter().collect();
            apply_jlap_patches(&applicable_patches, repo_data_json_path).await?;
        } else {
            return Err(JLAPError::NoHashFoundError);
        }

        Ok(())
    }

    /// Updates or creates the JLAP file we currently have cached
    ///
    /// If the file exists, then we update it otherwise, we just write an
    /// entire new file to cache.
    async fn save_jlap_cache(&self, response_text: &str) -> Result<(), JLAPError> {
        if self.repo_data_jlap_path.is_file() {
            update_jlap_file(&self.repo_data_jlap_path, response_text).await?;
            return Ok(());
        }

        if (cache_jlap_response(&self.repo_data_jlap_path, response_text).await).is_ok() {
            return Ok(());
        }

        Ok(())
    }
}

/// Fetches a JLAP response from server
pub async fn fetch_jlap(
    url: &str,
    client: &Client,
    range: Option<String>,
) -> Result<Response, reqwest::Error> {
    let request_builder = client.get(url);
    let mut headers = HeaderMap::default();

    if let Some(value) = range {
        headers.insert(
            reqwest::header::RANGE,
            HeaderValue::from_str(&value).unwrap(),
        );
    }

    request_builder.headers(headers).send().await
}

/// Builds a cache key used in storing JLAP cache
pub fn get_jlap_cache_path(subdir_url: &Url, cache_path: &Path) -> PathBuf {
    let cache_key = crate::utils::url_to_cache_filename(subdir_url);
    let cache_file_name = format!("{}.{}", cache_key, JLAP_FILE_SUFFIX);

    cache_path.join(cache_file_name)
}

/// Persist a JLAP file to the provided location
pub async fn cache_jlap_response(
    jlap_cache_path: &PathBuf,
    response_text: &str,
) -> Result<(), tokio::io::Error> {
    let mut jlap_file = tokio::fs::File::create(&jlap_cache_path).await?;
    jlap_file.write_all(response_text.as_bytes()).await?;

    Ok(())
}

/// Update an existing cached JLAP file
pub async fn update_jlap_file(jlap_file: &PathBuf, jlap_contents: &str) -> Result<(), JLAPError> {
    let parts: Vec<&str> = jlap_contents
        .split('\n')
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();

    // We only care about updating if the response is greater than 2 lines.
    // This means we received some new patches.
    if parts.len() > 2 {
        let mut cache_file = match tokio::fs::File::open(jlap_file).await {
            Ok(value) => value,
            Err(err) => return Err(JLAPError::FileSystemError(err)),
        };

        let mut contents = String::new();
        match cache_file.read_to_string(&mut contents).await {
            Ok(_) => (),
            Err(error) => return Err(JLAPError::FileSystemError(error)),
        }

        let mut current_parts: Vec<&str> = contents.split('\n').collect();
        current_parts.truncate(current_parts.len() - 2);
        current_parts.extend(parts);

        let updated_jlap = current_parts.join("\n").into_bytes();

        let mut updated_file = match tokio::fs::File::create(jlap_file).await {
            Ok(file) => file,
            Err(err) => return Err(JLAPError::FileSystemError(err)),
        };

        return match updated_file.write_all(&updated_jlap).await {
            Ok(_) => Ok(()),
            Err(err) => Err(JLAPError::FileSystemError(err)),
        };
    }

    Ok(())
}

/// Determines the byte offset to use for JLAP range requests
///
/// This function assumes we already have a locally cached version of the JLAP file
pub async fn get_jlap_request_range(
    jlap_cache: &PathBuf,
) -> Result<Option<String>, tokio::io::Error> {
    let mut cache_file = tokio::fs::File::open(jlap_cache).await?;
    let mut contents = String::from("");

    cache_file.read_to_string(&mut contents).await?;

    let mut lines: Vec<&str> = contents.split('\n').collect();
    let length = lines.len();

    if length >= JLAP_FOOTER_OFFSET {
        lines.truncate(length - JLAP_FOOTER_OFFSET);
        let patches = lines.join("\n");
        return Ok(Some(format!("bytes={}-", patches.into_bytes().len())));
    }

    // We default to starting from the beginning of the file.
    Ok(None)
}

fn parse_patch_json(line: &&str) -> Result<Patch, JLAPError> {
    serde_json::from_str(line).map_err(JLAPError::JSONParseError)
}

/// Given the path to a JLAP file, deserialize it and return the JLAPMetadata and Patch structs.
pub async fn get_jlap_metadata_and_patches(
    jlap_cache: &PathBuf,
) -> Result<(JLAPMetadata, Vec<Patch>), JLAPError> {
    let mut cache_file = match tokio::fs::File::open(jlap_cache).await {
        Ok(value) => value,
        Err(err) => return Err(JLAPError::FileSystemError(err)),
    };
    let mut contents = String::new();

    match cache_file.read_to_string(&mut contents).await {
        Ok(_) => (),
        Err(error) => return Err(JLAPError::FileSystemError(error)),
    }

    let parts: Vec<&str> = contents.split('\n').collect();
    let length = parts.len();

    if parts.len() > 2 {
        let metadata_line = parts[length - 2];

        let metadata: JLAPMetadata = match serde_json::from_str(metadata_line) {
            Ok(value) => value,
            Err(err) => return Err(JLAPError::JSONParseError(err)),
        };

        let patch_lines = parts[1..length - JLAP_FOOTER_OFFSET].iter();
        let patches: Result<Vec<Patch>, JLAPError> = patch_lines.map(parse_patch_json).collect();

        match patches {
            Ok(patches) => {
                if !patches.is_empty() {
                    Ok((metadata, patches))
                } else {
                    Err(JLAPError::NoPatchesFoundError)
                }
            }
            Err(error) => Err(error),
        }
    } else {
        Err(JLAPError::NoPatchesFoundError)
    }
}

/// Applies JLAP patches to a `repodata.json` file
///
/// This is a multi-step process that involves:
///
/// 1. Opening and parsing the current repodata file
/// 2. Applying patches to this repodata file
/// 3. Sorting the repodata file? (not sure)
/// 4. Saving this repodata file
pub async fn apply_jlap_patches(
    patches: &Vec<&Patch>,
    repo_data_path: &PathBuf,
) -> Result<(), JLAPError> {
    // Open and read the current repodata into a JSON doc
    let mut repo_data = match tokio::fs::File::open(repo_data_path).await {
        Ok(contents) => contents,
        Err(error) => return Err(JLAPError::FileSystemError(error)),
    };

    let mut repo_data_contents = String::new();
    match repo_data.read_to_string(&mut repo_data_contents).await {
        Ok(_) => (),
        Err(error) => return Err(JLAPError::FileSystemError(error)),
    }
    let mut doc = match serde_json::from_str(&repo_data_contents) {
        Ok(doc) => doc,
        Err(error) => return Err(JLAPError::JSONParseError(error)),
    };

    // Apply the patches we current have to it
    for patch in patches {
        match json_patch::patch(&mut doc, &patch.patch) {
            Ok(_) => (),
            Err(error) => return Err(JLAPError::JSONPatchError(error)),
        }
    }

    // Save the updated repodata JSON doc
    let mut updated_file = match tokio::fs::File::create(repo_data_path).await {
        Ok(file) => file,
        Err(error) => return Err(JLAPError::FileSystemError(error)),
    };

    let mut updated_json = match serde_json::to_string_pretty(&doc) {
        Ok(value) => value,
        Err(error) => return Err(JLAPError::JSONParseError(error)),
    };

    // We need to add an extra newline character to the end of our string so the hashes match 🤷‍
    updated_json.insert(updated_json.len(), '\n');

    match updated_file.write_all(&updated_json.into_bytes()).await {
        Ok(_) => Ok(()),
        Err(error) => Err(JLAPError::FileSystemError(error)),
    }
}

/// Finds the index of the of the most applicable patch to use
fn find_current_patch_index(patches: &[Patch], hash: &str) -> Option<usize> {
    for (idx, patch) in patches.iter().enumerate() {
        if hash == patch.from {
            return Some(idx);
        }
    }

    None
}

#[cfg(test)]
mod test {
    use super::{get_jlap_cache_path, JLAPManager};

    use crate::utils::simple_channel_server::SimpleChannelServer;

    use reqwest::Client;
    use tempfile::TempDir;

    const FAKE_REPO_DATA_INITIAL: &str = r#"{
  "info": {
    "subdir": "osx-64"
  },
  "packages.conda": {
    "zstd-1.5.4-hc035e20_0.conda": {
      "build": "hc035e20_0",
      "build_number": 0,
      "depends": [
        "libcxx >=14.0.6",
        "lz4-c >=1.9.4,<1.10.0a0",
        "xz >=5.2.10,<6.0a0",
        "zlib >=1.2.13,<1.3.0a0"
      ],
      "license": "BSD-3-Clause AND GPL-2.0-or-later",
      "license_family": "BSD",
      "md5": "f284fea068c51b1a0eaea3ac58c300c0",
      "name": "zstd",
      "sha256": "0af4513ef7ad7fa8854fa714130c25079f3744471fc106f47df80eb10c34429d",
      "size": 605550,
      "subdir": "osx-64",
      "timestamp": 1680034665911,
      "version": "1.5.4"
    }
  },
  "repodata_version": 1
}
"#;

    const FAKE_REPO_DATA_UPDATE_ONE: &str = r#"{
  "info": {
    "subdir": "osx-64"
  },
  "packages.conda": {
    "zstd-1.5.4-hc035e20_0.conda": {
      "build": "hc035e20_0",
      "build_number": 0,
      "depends": [
        "libcxx >=14.0.6",
        "lz4-c >=1.9.4,<1.10.0a0",
        "xz >=5.2.10,<6.0a0",
        "zlib >=1.2.13,<1.3.0a0"
      ],
      "license": "BSD-3-Clause AND GPL-2.0-or-later",
      "license_family": "BSD",
      "md5": "f284fea068c51b1a0eaea3ac58c300c0",
      "name": "zstd",
      "sha256": "0af4513ef7ad7fa8854fa714130c25079f3744471fc106f47df80eb10c34429d",
      "size": 605550,
      "subdir": "osx-64",
      "timestamp": 1680034665911,
      "version": "1.5.4"
    },
    "zstd-1.5.5-hc035e20_0.conda": {
      "build": "hc035e20_0",
      "build_number": 0,
      "depends": [
        "libcxx >=14.0.6",
        "lz4-c >=1.9.4,<1.10.0a0",
        "xz >=5.2.10,<6.0a0",
        "zlib >=1.2.13,<1.3.0a0"
      ],
      "license": "BSD-3-Clause AND GPL-2.0-or-later",
      "license_family": "BSD",
      "md5": "5e0b7ddb1b7dc6b630e1f9a03499c19c",
      "name": "zstd",
      "sha256": "5b192501744907b841de036bb89f5a2776b4cac5795ccc25dcaebeac784db038",
      "size": 622467,
      "subdir": "osx-64",
      "timestamp": 1681304595869,
      "version": "1.5.5"
    }
  },
  "repodata_version": 1
}
"#;

    const FAKE_REPO_DATA_UPDATE_TWO: &str = r#"{
  "info": {
    "subdir": "osx-64"
  },
  "packages.conda": {
    "zstd-1.5.4-hc035e20_0.conda": {
      "build": "hc035e20_0",
      "build_number": 0,
      "depends": [
        "libcxx >=14.0.6",
        "lz4-c >=1.9.4,<1.10.0a0",
        "xz >=5.2.10,<6.0a0",
        "zlib >=1.2.13,<1.3.0a0"
      ],
      "license": "BSD-3-Clause AND GPL-2.0-or-later",
      "license_family": "BSD",
      "md5": "f284fea068c51b1a0eaea3ac58c300c0",
      "name": "zstd",
      "sha256": "0af4513ef7ad7fa8854fa714130c25079f3744471fc106f47df80eb10c34429d",
      "size": 605550,
      "subdir": "osx-64",
      "timestamp": 1680034665911,
      "version": "1.5.4"
    },
    "zstd-1.5.5-hc035e20_0.conda": {
      "build": "hc035e20_0",
      "build_number": 0,
      "depends": [
        "libcxx >=14.0.6",
        "lz4-c >=1.9.4,<1.10.0a0",
        "xz >=5.2.10,<6.0a0",
        "zlib >=1.2.13,<1.3.0a0"
      ],
      "license": "BSD-3-Clause AND GPL-2.0-or-later",
      "license_family": "BSD",
      "md5": "5e0b7ddb1b7dc6b630e1f9a03499c19c",
      "name": "zstd",
      "sha256": "5b192501744907b841de036bb89f5a2776b4cac5795ccc25dcaebeac784db038",
      "size": 622467,
      "subdir": "osx-64",
      "timestamp": 1681304595869,
      "version": "1.5.5"
    },
    "zstd-static-1.4.5-hb1e8313_0.conda": {
      "build": "hb1e8313_0",
      "build_number": 0,
      "depends": [
        "libcxx >=10.0.0",
        "zstd 1.4.5 h41d2c2f_0"
      ],
      "license": "BSD 3-Clause",
      "md5": "5447986040e0b73d6c681a4d8f615d6c",
      "name": "zstd-static",
      "sha256": "3759ab53ff8320d35c6db00d34059ba99058eeec1cbdd0da968c5e12f73f7658",
      "size": 13930,
      "subdir": "osx-64",
      "timestamp": 1595965109852,
      "version": "1.4.5"
    }
  },
  "repodata_version": 1
}
"#;

    const FAKE_REPO_DATA_INITIAL_HASH: &str =
        "580100cb35459305eaaa31feeebacb06aad6422257754226d832e504666fc1c6";

    const FAKE_REPO_DATA_UPDATE_ONE_HASH: &str =
        "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9";

    const FAKE_JLAP_DATA_INITIAL: &str = r#"0000000000000000000000000000000000000000000000000000000000000000
{"to": "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9", "from": "580100cb35459305eaaa31feeebacb06aad6422257754226d832e504666fc1c6", "patch": [{"op": "add", "path": "/packages.conda/zstd-1.5.5-hc035e20_0.conda", "value": {"build": "hc035e20_0","build_number": 0,"depends": ["libcxx >=14.0.6","lz4-c >=1.9.4,<1.10.0a0","xz >=5.2.10,<6.0a0","zlib >=1.2.13,<1.3.0a0"],"license": "BSD-3-Clause AND GPL-2.0-or-later","license_family": "BSD","md5": "5e0b7ddb1b7dc6b630e1f9a03499c19c","name": "zstd","sha256": "5b192501744907b841de036bb89f5a2776b4cac5795ccc25dcaebeac784db038","size": 622467,"subdir": "osx-64","timestamp": 1681304595869, "version": "1.5.5"}}]}
{"url": "repodata.json", "latest": "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9"}
c540a2ab0ab4674dada39063205a109d26027a55bd8d7a5a5b711be03ffc3a9d"#;

    const FAKE_JLAP_DATA_UPDATE_ONE: &str = r#"0000000000000000000000000000000000000000000000000000000000000000
{"to": "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9", "from": "580100cb35459305eaaa31feeebacb06aad6422257754226d832e504666fc1c6", "patch": [{"op": "add", "path": "/packages.conda/zstd-1.5.5-hc035e20_0.conda", "value": {"build": "hc035e20_0","build_number": 0,"depends": ["libcxx >=14.0.6","lz4-c >=1.9.4,<1.10.0a0","xz >=5.2.10,<6.0a0","zlib >=1.2.13,<1.3.0a0"],"license": "BSD-3-Clause AND GPL-2.0-or-later","license_family": "BSD","md5": "5e0b7ddb1b7dc6b630e1f9a03499c19c","name": "zstd","sha256": "5b192501744907b841de036bb89f5a2776b4cac5795ccc25dcaebeac784db038","size": 622467,"subdir": "osx-64","timestamp": 1681304595869, "version": "1.5.5"}}]}
{"to": "160b529c5f72b9755f951c1b282705d49d319a5f2f80b33fb1a670d02ddeacf9", "from": "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9", "patch": [{"op": "add", "path": "/packages.conda/zstd-static-1.4.5-hb1e8313_0.conda", "value": {"build": "hb1e8313_0", "build_number": 0, "depends": ["libcxx >=10.0.0", "zstd 1.4.5 h41d2c2f_0"], "license": "BSD 3-Clause", "md5": "5447986040e0b73d6c681a4d8f615d6c", "name": "zstd-static", "sha256": "3759ab53ff8320d35c6db00d34059ba99058eeec1cbdd0da968c5e12f73f7658", "size": 13930, "subdir": "osx-64", "timestamp": 1595965109852, "version": "1.4.5"}}]}
{"url": "repodata.json", "latest": "160b529c5f72b9755f951c1b282705d49d319a5f2f80b33fb1a670d02ddeacf9"}
c540a2ab0ab4674dada39063205a109d26027a55bd8d7a5a5b711be03ffc3a9d"#;

    #[tokio::test]
    /// Performs a test to make sure that patches can be applied when we retrieve
    /// a "fresh" (i.e. no bytes offset) version of the JLAP file.
    pub async fn test_patch_repo_data() {
        // Begin setup (this can probably be put somewhere else)

        // Create a directory with some repodata.
        let subdir_path = TempDir::new().unwrap();
        let server = SimpleChannelServer::new(subdir_path.path());

        // Add files we need to request to the server
        tokio::fs::write(
            subdir_path.path().join("repodata.jlap"),
            FAKE_JLAP_DATA_INITIAL,
        )
        .await
        .unwrap();

        // Create our cache location and files we need there
        let cache_dir = TempDir::new().unwrap();

        // This is the existing `repodata.json` file that will be patched
        let cache_key = crate::utils::url_to_cache_filename(
            &server
                .url()
                .join("repodata.json")
                .expect("file name is valid"),
        );
        let repo_data_json_path = cache_dir.path().join(format!("{}.json", cache_key));
        tokio::fs::write(repo_data_json_path.clone(), FAKE_REPO_DATA_INITIAL)
            .await
            .unwrap();

        // HTTP client we need to initialize the JLAPManager object.
        let client = Client::default();

        // End setup

        // Run the code under test
        let manager = JLAPManager::new(
            server.url(),
            &client,
            cache_dir.path(),
            Some(FAKE_REPO_DATA_INITIAL_HASH.to_string()),
        )
        .await;

        let jlap_cache = manager.repo_data_jlap_path.clone();

        manager
            .patch_repo_data(&repo_data_json_path.clone())
            .await
            .unwrap();

        // Make assertions
        let repo_data = tokio::fs::read_to_string(repo_data_json_path)
            .await
            .unwrap();
        let jlap_data = tokio::fs::read_to_string(jlap_cache).await.unwrap();

        assert_eq!(repo_data, FAKE_REPO_DATA_UPDATE_ONE);
        assert_eq!(jlap_data, FAKE_JLAP_DATA_INITIAL);
    }

    #[tokio::test]
    /// Performs a test to make sure that patches can be applied when we retrieve
    /// a "partial" (i.e. one with a byte offset) version of the JLAP file.
    pub async fn test_patch_repo_data_partial() {
        // Begin setup (this can probably be put somewhere else)

        // Create a directory with some repodata.
        let subdir_path = TempDir::new().unwrap();
        let server = SimpleChannelServer::new(subdir_path.path());

        // Add files we need to request to the server
        tokio::fs::write(
            subdir_path.path().join("repodata.jlap"),
            FAKE_JLAP_DATA_UPDATE_ONE,
        )
        .await
        .unwrap();

        // Create our cache location and files we need there
        let cache_dir = TempDir::new().unwrap();

        // This is the existing `repodata.json` file that will be patched
        let cache_key = crate::utils::url_to_cache_filename(
            &server
                .url()
                .join("repodata.json")
                .expect("file name is valid"),
        );
        let repo_data_json_path = cache_dir.path().join(format!("{}.json", cache_key));
        tokio::fs::write(repo_data_json_path.clone(), FAKE_REPO_DATA_UPDATE_ONE)
            .await
            .unwrap();

        // Write the an out of data version of JLAP to the cache
        let jlap_cache = get_jlap_cache_path(&server.url(), cache_dir.path());
        tokio::fs::write(jlap_cache, FAKE_JLAP_DATA_INITIAL)
            .await
            .unwrap();

        // HTTP client we need to initialize the JLAPManager object.
        let client = Client::default();

        // End setup

        // Run the code under test
        let manager = JLAPManager::new(
            server.url(),
            &client,
            cache_dir.path(),
            Some(FAKE_REPO_DATA_INITIAL_HASH.to_string()),
        )
        .await;

        let jlap_cache = manager.repo_data_jlap_path.clone();

        manager
            .patch_repo_data(&repo_data_json_path.clone())
            .await
            .unwrap();

        // Make assertions
        let repo_data = tokio::fs::read_to_string(repo_data_json_path)
            .await
            .unwrap();
        let jlap_data = tokio::fs::read_to_string(jlap_cache).await.unwrap();

        assert_eq!(repo_data, FAKE_REPO_DATA_UPDATE_TWO);
        assert_eq!(jlap_data, FAKE_JLAP_DATA_UPDATE_ONE);
    }
}
