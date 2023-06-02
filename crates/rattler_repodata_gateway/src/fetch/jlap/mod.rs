//! # JLAP
//!
//! This module contains functions and data types for downloading and applying patches from JLAP
//! files.
//!
//! JLAP files provide a way to incrementally retrieve and build the `repodata.json` files
//! that conda compatible applications use to query conda packages. For more information about
//! how this file format works, please read this CEP proposal:
//!
//! - <https://github.com/conda-incubator/ceps/pull/20/files>
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
//!
//! use rattler_digest::{compute_bytes_digest, Blake2b256};
//! use rattler_repodata_gateway::fetch::jlap::{patch_repo_data, JLAPState, RepoDataState};
//!
//! #[tokio::main]
//! pub async fn main() {
//!     let subdir_url = Url::parse("https://conda.anaconda.org/conda-forge/osx-64/").unwrap();
//!     let client = Client::new();
//!     let cache = Path::new("./cache");
//!     let current_repo_data = cache.join("c93ef9c9.json");
//!
//!     let repo_data_state: RepoDataState =  serde_json::from_str(r#"{
//!        "url": "https://conda.anaconda.org/conda-forge/osx-64/repodata.json.zst",
//!        "etag": "W/\"49aa6d9ea6f3285efe657780a7c8cd58\"",
//!        "mod": "Tue, 30 May 2023 20:03:48 GMT",
//!        "cache_control": "public, max-age=30",
//!        "mtime_ns": 1685509481332236078,
//!        "size": 38317593,
//!        "blake2_hash": "580100cb35459305eaaa31feeebacb06aad6422257754226d832e504666fc1c6",
//!        "has_zst": {
//!          "value": true,
//!          "last_checked": "2023-05-21T12:14:21.904003Z"
//!        },
//!        "has_bz2": {
//!          "value": true,
//!          "last_checked": "2023-05-21T12:14:21.904003Z"
//!        },
//!        "has_jlap": {
//!          "value": true,
//!          "last_checked": "2023-05-21T12:14:21.903512Z"
//!        },
//!        "jlap": {
//!          "iv": "0000000000000000000000000000000000000000000000000000000000000000",
//!          "pos": 0,
//!          "footer": {
//!            "url": "repodata.json",
//!            "latest": "580100cb35459305eaaa31feeebacb06aad6422257754226d832e504666fc1c6"
//!          }
//!        }
//!      }"#).unwrap();
//!
//!     // Patches `current_repo_data` and returns an updated JLAP state object
//!     let updated_jlap_state = patch_repo_data(
//!         &client,
//!         subdir_url,
//!         repo_data_state,
//!         &current_repo_data
//!     ).await.unwrap();
//!
//!     // Now we can use the `updated_jlap_state` object to update our `.state.json` file
//! }
//! ```
//!
//! ## TODO
//!
//! The following items still need to be implemented before this module should be considered
//! complete:
//!  - Use the checksum to validate our JLAP file after we update it
//!  - Our tests do not exhaustively cover our error states. Some of these are pretty easy to
//!    trigger (e.g. invalid JLAP file or invalid JSON within), so we should definitely make
//!    tests for them.

use blake2::digest::Output;
use rattler_digest::{compute_bytes_digest, Blake2b256};
use reqwest::{
    header::{HeaderMap, HeaderValue},
    Client, Response,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::str;
use tokio::io::AsyncWriteExt;
use url::Url;

use crate::fetch::cache;
pub use crate::fetch::cache::{JLAPFooter, JLAPState, RepoDataState};

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

    #[error("Hash from the JLAP metadata and hash from updated repodata file do not match")]
    /// Error when we have mismatched hash values after updating our `repodata.json` file
    /// At this point, we should consider the `repodata.json` file corrupted and fetch a new
    /// version from the server.
    HashesNotMatchingError,
}

/// Represents the numerous patches found in a JLAP file which makes up a majority
/// of the file
#[derive(Serialize, Deserialize, Debug)]
pub struct Patch {
    /// Next hash of `repodata.json` file
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "cache::deserialize_blake2_hash",
        serialize_with = "cache::serialize_blake2_hash"
    )]
    pub to: Option<Output<Blake2b256>>,

    /// Previous hash of `repodata.json` file
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "cache::deserialize_blake2_hash",
        serialize_with = "cache::serialize_blake2_hash"
    )]
    pub from: Option<Output<Blake2b256>>,

    /// Patches to apply to current `repodata.json` file
    pub patch: json_patch::Patch, // [] is a valid, empty patch
}

/// Attempts to patch a current `repodata.json` file
///
/// This method first makes a request to fetch JLAP data we need. It relies on the information we
/// pass via the `jlap_state` argument to retrieve the correct response.
///
/// After this, it will apply JSON patches to the file located at `repo_data_json_path`.
/// At the end, we compare the new `blake2b` hash with what was listed in the JLAP metadata to
/// ensure the file is correct.
///
/// The return value is the updated `blake2b` hash.
pub async fn patch_repo_data(
    client: &Client,
    subdir_url: Url,
    repo_data_state: RepoDataState,
    repo_data_json_path: &Path,
) -> Result<JLAPState, JLAPError> {
    let jlap_state = repo_data_state.jlap.unwrap_or_default();
    let jlap_url = subdir_url.join(JLAP_FILE_NAME).unwrap();
    let range = format!("bytes={}-", jlap_state.pos);

    let response = match fetch_jlap(jlap_url.as_str(), client, range).await {
        Ok(response) => response,
        Err(error) => {
            return Err(JLAPError::HTTPError(error));
        }
    };

    let response_text = response.text().await.unwrap();
    let new_initial_vector = get_initial_vector(&response_text).unwrap_or(jlap_state.iv);
    let new_bytes_offset = calculate_bytes_offset(&response_text) as u64;

    // Get the patches and apply the new patches if any found
    let (footer, patches) = get_jlap_data(&response_text).await?;

    let hash = repo_data_state.blake2_hash.unwrap_or_default();
    let latest_hash = footer.latest.unwrap_or_default();

    // We already have the latest version; return early because there's nothing to do
    if latest_hash == hash {
        let new_state = JLAPState {
            pos: jlap_state.pos + new_bytes_offset,
            iv: new_initial_vector,
            footer,
        };
        return Ok(new_state);
    }

    // We use the current hash to find which patches we need to apply
    let current_idx = find_current_patch_index(&patches, hash);

    return if let Some(idx) = current_idx {
        let applicable_patches: Vec<&Patch> = patches[idx..patches.len()].iter().collect();
        let new_hash = apply_jlap_patches(&applicable_patches, repo_data_json_path).await?;

        if new_hash != latest_hash {
            return Err(JLAPError::HashesNotMatchingError);
        }

        let new_bytes_offset = calculate_bytes_offset(&response_text) as u64;

        let new_state = JLAPState {
            pos: jlap_state.pos + new_bytes_offset,
            iv: new_initial_vector,
            footer,
        };

        Ok(new_state)
    } else {
        Err(JLAPError::NoHashFoundError)
    };
}

/// Fetches a JLAP response from server
pub async fn fetch_jlap(
    url: &str,
    client: &Client,
    range: String,
) -> Result<Response, reqwest::Error> {
    let request_builder = client.get(url);
    let mut headers = HeaderMap::default();

    headers.insert(
        reqwest::header::RANGE,
        HeaderValue::from_str(&range).unwrap(),
    );

    request_builder.headers(headers).send().await
}

/// Utility function to calculate the bytes offset for the next JLAP request
pub fn calculate_bytes_offset(jlap_response: &str) -> usize {
    let mut lines: Vec<&str> = jlap_response.split('\n').collect();
    let length = lines.len();

    if length >= JLAP_FOOTER_OFFSET {
        lines.truncate(length - JLAP_FOOTER_OFFSET);
        let patches = lines.join("\n");
        return patches.into_bytes().len();
    }

    0
}

fn parse_patch_json(line: &&str) -> Result<Patch, JLAPError> {
    serde_json::from_str(line).map_err(JLAPError::JSONParseError)
}

/// Builds a new JLAP object based on the response
pub async fn get_jlap_data(jlap_response: &str) -> Result<(JLAPFooter, Vec<Patch>), JLAPError> {
    let parts: Vec<&str> = jlap_response.split('\n').collect();
    let length = parts.len();

    if parts.len() > 2 {
        let footer = parts[length - 2];

        let metadata: JLAPFooter = match serde_json::from_str(footer) {
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
/// 3. Saving this repodata file to disk
/// 4. Generating a new `blake2b` hash
///
/// The return value is the `blake2b` hash we used to verify the updated file's contents.
pub async fn apply_jlap_patches(
    patches: &Vec<&Patch>,
    repo_data_path: &Path,
) -> Result<Output<Blake2b256>, JLAPError> {
    // Open and read the current repodata into a JSON doc
    let repo_data_contents = match tokio::fs::read_to_string(repo_data_path).await {
        Ok(contents) => contents,
        Err(error) => return Err(JLAPError::FileSystemError(error)),
    };

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
    let content = updated_json.into_bytes();

    match updated_file.write_all(&content).await {
        Ok(_) => Ok(compute_bytes_digest::<Blake2b256>(content)),
        Err(error) => Err(JLAPError::FileSystemError(error)),
    }
}

/// Finds the index of the of the most applicable patch to use
fn find_current_patch_index(patches: &[Patch], hash: Output<Blake2b256>) -> Option<usize> {
    for (idx, patch) in patches.iter().enumerate() {
        if hash == patch.from.unwrap_or_default() {
            return Some(idx);
        }
    }

    None
}

/// Finds the initial vector in the JLAP response; this is the first line of a full request
fn get_initial_vector(jlap_response: &str) -> Option<String> {
    let parts: Vec<&str> = jlap_response.split('\n').collect();

    if !parts.is_empty() {
        Some(parts[0].to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod test {
    use super::patch_repo_data;
    use std::path::PathBuf;

    use crate::fetch::cache::RepoDataState;
    use crate::utils::simple_channel_server::SimpleChannelServer;

    use rattler_digest::{parse_digest_from_hex, Blake2b256};
    use reqwest::Client;
    use tempfile::TempDir;
    use url::Url;

    const FAKE_STATE_DATA_INITIAL: &str = r#"{
  "url": "https://repo.example.com/pkgs/main/osx-64/repodata.json.zst",
  "etag": "W/\"49aa6d9ea6f3285efe657780a7c8cd58\"",
  "mod": "Tue, 30 May 2023 20:03:48 GMT",
  "cache_control": "public, max-age=30",
  "mtime_ns": 1685509481332236078,
  "size": 38317593,
  "blake2_hash": "580100cb35459305eaaa31feeebacb06aad6422257754226d832e504666fc1c6",
  "has_zst": {
    "value": true,
    "last_checked": "2023-05-21T12:14:21.904003Z"
  },
  "has_bz2": {
    "value": true,
    "last_checked": "2023-05-21T12:14:21.904003Z"
  },
  "has_jlap": {
    "value": true,
    "last_checked": "2023-05-21T12:14:21.903512Z"
  },
  "jlap": {
    "iv": "0000000000000000000000000000000000000000000000000000000000000000",
    "pos": 0,
    "footer": {
      "url": "repodata.json",
      "latest": "580100cb35459305eaaa31feeebacb06aad6422257754226d832e504666fc1c6"
    }
  }
}"#;

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

    const FAKE_REPO_DATA_UPDATE_ONE_HASH: &str =
        "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9";

    const FAKE_REPO_DATA_UPDATE_TWO_HASH: &str =
        "160b529c5f72b9755f951c1b282705d49d319a5f2f80b33fb1a670d02ddeacf9";

    const FAKE_JLAP_DATA_INITIAL: &str = r#"0000000000000000000000000000000000000000000000000000000000000000
{"to": "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9", "from": "580100cb35459305eaaa31feeebacb06aad6422257754226d832e504666fc1c6", "patch": [{"op": "add", "path": "/packages.conda/zstd-1.5.5-hc035e20_0.conda", "value": {"build": "hc035e20_0","build_number": 0,"depends": ["libcxx >=14.0.6","lz4-c >=1.9.4,<1.10.0a0","xz >=5.2.10,<6.0a0","zlib >=1.2.13,<1.3.0a0"],"license": "BSD-3-Clause AND GPL-2.0-or-later","license_family": "BSD","md5": "5e0b7ddb1b7dc6b630e1f9a03499c19c","name": "zstd","sha256": "5b192501744907b841de036bb89f5a2776b4cac5795ccc25dcaebeac784db038","size": 622467,"subdir": "osx-64","timestamp": 1681304595869, "version": "1.5.5"}}]}
{"url": "repodata.json", "latest": "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9"}
c540a2ab0ab4674dada39063205a109d26027a55bd8d7a5a5b711be03ffc3a9d"#;

    const FAKE_JLAP_DATA_UPDATE_ONE: &str = r#"0000000000000000000000000000000000000000000000000000000000000000
{"to": "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9", "from": "580100cb35459305eaaa31feeebacb06aad6422257754226d832e504666fc1c6", "patch": [{"op": "add", "path": "/packages.conda/zstd-1.5.5-hc035e20_0.conda", "value": {"build": "hc035e20_0","build_number": 0,"depends": ["libcxx >=14.0.6","lz4-c >=1.9.4,<1.10.0a0","xz >=5.2.10,<6.0a0","zlib >=1.2.13,<1.3.0a0"],"license": "BSD-3-Clause AND GPL-2.0-or-later","license_family": "BSD","md5": "5e0b7ddb1b7dc6b630e1f9a03499c19c","name": "zstd","sha256": "5b192501744907b841de036bb89f5a2776b4cac5795ccc25dcaebeac784db038","size": 622467,"subdir": "osx-64","timestamp": 1681304595869, "version": "1.5.5"}}]}
{"to": "160b529c5f72b9755f951c1b282705d49d319a5f2f80b33fb1a670d02ddeacf9", "from": "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9", "patch": [{"op": "add", "path": "/packages.conda/zstd-static-1.4.5-hb1e8313_0.conda", "value": {"build": "hb1e8313_0", "build_number": 0, "depends": ["libcxx >=10.0.0", "zstd 1.4.5 h41d2c2f_0"], "license": "BSD 3-Clause", "md5": "5447986040e0b73d6c681a4d8f615d6c", "name": "zstd-static", "sha256": "3759ab53ff8320d35c6db00d34059ba99058eeec1cbdd0da968c5e12f73f7658", "size": 13930, "subdir": "osx-64", "timestamp": 1595965109852, "version": "1.4.5"}}]}
{"url": "repodata.json", "latest": "160b529c5f72b9755f951c1b282705d49d319a5f2f80b33fb1a670d02ddeacf9"}
c540a2ab0ab4674dada39063205a109d26027a55bd8d7a5a5b711be03ffc3a9d"#;

    /// Writes the desired files to the "server" environment
    async fn setup_server_environment(
        server_repo_data: Option<&str>,
        server_jlap: Option<&str>,
    ) -> TempDir {
        // Create a directory with some repodata; this is the "server" data
        let subdir_path = TempDir::new().unwrap();

        if let Some(content) = server_jlap {
            // Add files we need to request to the server
            tokio::fs::write(subdir_path.path().join("repodata.jlap"), content)
                .await
                .unwrap();
        }

        if let Some(content) = server_repo_data {
            // Add files we need to request to the server
            tokio::fs::write(subdir_path.path().join("repodata.json"), content)
                .await
                .unwrap();
        }

        subdir_path
    }

    /// Writes the desired files to the "client" environment
    async fn setup_client_environment(
        server_url: &Url,
        cache_repo_data: Option<&str>,
    ) -> (TempDir, PathBuf) {
        // Create our cache location and files we need there; this is our "cache" location
        let cache_dir = TempDir::new().unwrap();

        // This is the existing `repodata.json` file that will be patched
        let cache_key = crate::utils::url_to_cache_filename(
            &server_url
                .join("repodata.json")
                .expect("file name is valid"),
        );
        let cache_repo_data_path = cache_dir.path().join(format!("{}.json", cache_key));

        if let Some(content) = cache_repo_data {
            tokio::fs::write(cache_repo_data_path.clone(), content)
                .await
                .unwrap();
        }

        (cache_dir, cache_repo_data_path)
    }

    #[tokio::test]
    /// Performs a test to make sure that patches can be applied when we retrieve
    /// a "fresh" (i.e. no bytes offset) version of the JLAP file.
    pub async fn test_patch_repo_data() {
        // Begin setup
        let subdir_path = setup_server_environment(None, Some(FAKE_JLAP_DATA_INITIAL)).await;
        let server = SimpleChannelServer::new(subdir_path.path());
        let server_url = server.url();

        let (_cache_dir, cache_repo_data_path) =
            setup_client_environment(&server_url, Some(FAKE_REPO_DATA_INITIAL)).await;

        let client = Client::default();

        let repo_data_state: RepoDataState = serde_json::from_str(FAKE_STATE_DATA_INITIAL).unwrap();
        // End setup

        let updated_jlap_state =
            patch_repo_data(&client, server_url, repo_data_state, &cache_repo_data_path)
                .await
                .unwrap();

        // Make assertions
        let repo_data = tokio::fs::read_to_string(cache_repo_data_path)
            .await
            .unwrap();

        // Ensure the repo data was updated appropriately
        assert_eq!(repo_data, FAKE_REPO_DATA_UPDATE_ONE);

        // Ensure the the updated JLAP state matches what it should
        assert_eq!(updated_jlap_state.pos, 737);
        assert_eq!(
            updated_jlap_state.footer.latest.unwrap_or_default(),
            parse_digest_from_hex::<Blake2b256>(FAKE_REPO_DATA_UPDATE_ONE_HASH).unwrap()
        );
    }

    #[tokio::test]
    /// Performs a test to make sure that patches can be applied when we retrieve
    /// a "partial" (i.e. one with a byte offset) version of the JLAP file.
    pub async fn test_patch_repo_data_partial() {
        // Begin setup
        let subdir_path = setup_server_environment(None, Some(FAKE_JLAP_DATA_UPDATE_ONE)).await;
        let server = SimpleChannelServer::new(subdir_path.path());
        let server_url = server.url();

        let (_cache_dir, cache_repo_data_path) =
            setup_client_environment(&server_url, Some(FAKE_REPO_DATA_UPDATE_ONE)).await;

        let client = Client::default();

        let repo_data_state: RepoDataState = serde_json::from_str(FAKE_STATE_DATA_INITIAL).unwrap();
        // End setup

        // Run the code under test
        let updated_jlap_state =
            patch_repo_data(&client, server_url, repo_data_state, &cache_repo_data_path)
                .await
                .unwrap();

        // Make assertions
        let repo_data = tokio::fs::read_to_string(cache_repo_data_path)
            .await
            .unwrap();

        // Ensure the repo data was updated appropriately
        assert_eq!(repo_data, FAKE_REPO_DATA_UPDATE_TWO);

        // Ensure the the updated JLAP state matches what it should
        assert_eq!(updated_jlap_state.pos, 1340);
        assert_eq!(
            updated_jlap_state.footer.latest.unwrap_or_default(),
            parse_digest_from_hex::<Blake2b256>(FAKE_REPO_DATA_UPDATE_TWO_HASH).unwrap()
        );
    }
}
