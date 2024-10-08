//! # JLAP
//!
//! This module contains functions and data types for downloading and applying patches from JLAP
//! files.
//!
//! JLAP files provide a way to incrementally retrieve and build the `repodata.json` files
//! that conda compatible applications use to query conda packages. For more information about
//! how this file format works, please read this CEP proposal:
//!
//! - <https://github.com/conda/ceps/pull/20/files>
//!
//! ## Example
//!
//! The recommended way to use this module is by using the [`patch_repo_data`] function. This
//! function first makes a request to fetch any new JLAP patches, validates the request to make
//! sure we are applying the correct patches and then actually applies the patches to the provided
//! `repodata.json` cache file.
//!
//! Below is an example of how to call this function:
//!
//! ```no_run
//! use std::{path::Path};
//! use rattler_networking::AuthenticationMiddleware;
//! use url::Url;
//! use std::sync::Arc;
//!
//! use rattler_repodata_gateway::fetch::jlap::{patch_repo_data, RepoDataState};
//!
//! #[tokio::main]
//! pub async fn main() {
//!     let subdir_url = Url::parse("https://conda.anaconda.org/conda-forge/osx-64/").unwrap();
//!     let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
//!         .with_arc(Arc::new(AuthenticationMiddleware::default()))
//!         .build();
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
//!          "iv": "5a4c42192a69299198bd8cfc85146d725d0dcc24a4e50f6eab383bc37cab2d2d",
//!          "pos": 922035,
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
//!         &current_repo_data,
//!         None
//!     ).await.unwrap();
//!
//!     // Now we can use the `updated_jlap_state` object to update our `.info.json` file
//! }
//! ```
//!

use blake2::digest::Output;
use blake2::digest::{FixedOutput, Update};
use fs_err as fs;
use rattler_digest::{
    parse_digest_from_hex, serde::SerializableHash, Blake2b256, Blake2b256Hash, Blake2bMac256,
};
use rattler_redaction::Redact;
use reqwest::{
    header::{HeaderMap, HeaderValue},
    Response, StatusCode,
};
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_with::serde_as;
use std::iter::Iterator;
use std::path::Path;
use std::str;
use std::str::FromStr;
use std::sync::Arc;
use tempfile::NamedTempFile;
use url::Url;

pub use crate::fetch::cache::{JLAPFooter, JLAPState, RepoDataState};
use crate::reporter::ResponseReporterExt;
use crate::Reporter;
use simple_spawn_blocking::{tokio::run_blocking_task, Cancelled};

/// File suffix for JLAP file
pub const JLAP_FILE_SUFFIX: &str = "jlap";

/// File name of JLAP file
pub const JLAP_FILE_NAME: &str = "repodata.jlap";

/// File suffix for JLAP files
pub const JLAP_FOOTER_OFFSET: usize = 2;

/// Default position for JLAP requests
pub const JLAP_START_POSITION: u64 = 0;

/// Default initialization vector for JLAP requests
pub const JLAP_START_INITIALIZATION_VECTOR: &[u8] = &[0; 32];

/// Represents the variety of errors that we come across while processing JLAP files
#[derive(Debug, thiserror::Error)]
pub enum JLAPError {
    #[error(transparent)]
    /// Pass-thru for JSON errors found while parsing JLAP file
    JSONParse(serde_json::Error),

    #[error(transparent)]
    /// Pass-thru for JSON errors found while patching
    JSONPatch(json_patch::PatchError),

    #[error(transparent)]
    /// Pass-thru for HTTP errors encountered while requesting JLAP
    HTTP(reqwest_middleware::Error),

    #[error(transparent)]
    /// Pass-thru for file system errors encountered while requesting JLAP
    FileSystem(tokio::io::Error),

    #[error("No matching hashes can be found in the JLAP file")]
    /// Error returned when none of the patches match the hash of our current `repodata.json`
    /// This can also happen when the list of generated hashes for checksums is too short and
    /// and we are unable to find any hashes in the vector.
    NoHashFound,

    #[error("A mismatch occurred when validating the checksum on the JLAP response")]
    /// Error returned when we are unable to validate the checksum on the JLAP response.
    /// The checksum is the last line of the response.
    ChecksumMismatch,

    #[error("An error occurred while parsing the checksum on the JLAP response")]
    /// Error returned when parsing the checksum at the very end of the JLAP response occurs
    /// This should be seldom and might indicate an error on the server.
    ChecksumParse,

    #[error("The JLAP response was empty and we unable to parse it")]
    /// Error return if we cannot find anything inside the actual JLAP response.
    /// This indicates that we need to reset the values for JLAP in our cache.
    InvalidResponse,

    /// The operation was cancelled
    #[error("the operation was cancelled")]
    Cancelled,
}

impl From<Cancelled> for JLAPError {
    fn from(_: Cancelled) -> Self {
        JLAPError::Cancelled
    }
}

impl From<reqwest_middleware::Error> for JLAPError {
    fn from(value: reqwest_middleware::Error) -> Self {
        Self::HTTP(value.redact())
    }
}

impl From<reqwest::Error> for JLAPError {
    fn from(value: reqwest::Error) -> Self {
        Self::HTTP(value.redact().into())
    }
}

/// Represents the numerous patches found in a JLAP file which makes up a majority
/// of the file
#[serde_as]
#[derive(Serialize, Deserialize, Debug)]
pub struct Patch {
    /// Next hash of `repodata.json` file
    #[serde_as(as = "SerializableHash::<rattler_digest::Blake2b256>")]
    pub to: Output<Blake2b256>,

    /// Previous hash of `repodata.json` file
    #[serde_as(as = "SerializableHash::<rattler_digest::Blake2b256>")]
    pub from: Output<Blake2b256>,

    /// Patches to apply to current `repodata.json` file
    pub patch: json_patch::Patch, // [] is a valid, empty patch
}

impl FromStr for Patch {
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
    }
}

/// Represents a single JLAP response
///
/// All of the data contained in this struct is everything we can determine from the
/// JLAP response itself.
#[derive(Debug)]
pub struct JLAPResponse<'a> {
    /// First line of the JLAP response
    initialization_vector: Vec<u8>,

    /// List of patches parsed from the JLAP Response
    patches: Arc<[Patch]>,

    /// Footer of the request which contains data like the latest hash
    footer: JLAPFooter,

    /// Checksum located at the end of the request
    checksum: Output<Blake2b256>,

    /// Position to use for the next JLAP request
    new_position: u64,

    /// All the lines of the JLAP request (raw response, minus '\n' characters
    lines: Vec<&'a str>,

    /// Offset to use when iterating through JLAP response lines (i.e. position where the patches
    /// begin).
    offset: usize,
}

impl<'a> JLAPResponse<'a> {
    /// Parses a JLAP object from Response string.
    ///
    /// To successfully parse the response, it needs to at least be three lines long.
    /// This would include a [`Patch`], [`JLAPFooter`] and a `checksum`.
    ///
    /// On top of the response string, we also pass in a value for `initialization_vector`.
    /// If the response is not partial (i.e. it does not begin with an initialization vector,
    /// then this is the value we use. This is an important value for validating the checksum.
    pub fn new(response: &'a str, state: &JLAPState) -> Result<Self, JLAPError> {
        let lines: Vec<&str> = response.lines().collect();
        let length = lines.len();
        let mut patches: Vec<Patch> = vec![];
        let mut new_position = state.position;

        if length < JLAP_FOOTER_OFFSET {
            return Err(JLAPError::InvalidResponse);
        }

        // The first line can either be a valid hex string or JSON. We determine the `offset`
        // value to use and the value of the initialization vector here.
        let mut offset = 0;
        let initialization_vector = match hex::decode(lines[0]) {
            Ok(value) => {
                offset = 1;
                value
            }
            Err(_) => state.initialization_vector.clone(),
        };

        let footer = lines[length - 2];
        let footer: JLAPFooter = match serde_json::from_str(footer) {
            Ok(value) => value,
            Err(err) => return Err(JLAPError::JSONParse(err)),
        };

        let checksum = match parse_digest_from_hex::<Blake2b256>(lines[length - 1]) {
            Some(value) => value,
            None => return Err(JLAPError::ChecksumParse),
        };

        // This indicates we have patch lines to parse; patch lines for JLAP responses are optional
        // (i.e. no new data means no new patches)
        if lines.len() > JLAP_FOOTER_OFFSET {
            new_position += get_bytes_offset(&lines);

            let patch_lines = lines[offset..length - JLAP_FOOTER_OFFSET].iter();
            let patches_result: Result<Vec<Patch>, JLAPError> = patch_lines
                .map(|x| Patch::from_str(x).map_err(JLAPError::JSONParse))
                .collect();

            patches = match patches_result {
                Ok(patches) => patches,
                Err(error) => return Err(error),
            };
        }

        Ok(JLAPResponse {
            initialization_vector,
            patches: patches.into(),
            footer,
            checksum,
            new_position,
            lines,
            offset,
        })
    }

    /// Applies patches to a `repo_data_json_path` file provided using the `hash` value to
    /// find the correct ones to apply.
    pub async fn apply(
        &self,
        repo_data_json_path: &Path,
        hash: Output<Blake2b256>,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> Result<Blake2b256Hash, JLAPError> {
        // We use the current hash to find which patches we need to apply
        let current_idx = self.patches.iter().position(|patch| patch.from == hash);
        let Some(idx) = current_idx else {
            return Err(JLAPError::NoHashFound);
        };

        // Apply the patches on a blocking thread. Applying the patches is a relatively CPU intense
        // operation and we don't want to block the tokio runtime.
        let repo_data_path = self.patches.clone();
        let repo_data_json_path = repo_data_json_path.to_path_buf();
        run_blocking_task(move || {
            apply_jlap_patches(repo_data_path, idx, &repo_data_json_path, reporter)
        })
        .await
    }

    /// Returns a new [`JLAPState`] based on values in [`JLAPResponse`] struct
    ///
    /// We accept `position` as an argument because it is not derived from the JLAP response.
    ///
    /// The `initialization_vector` value is optionally passed in because we may wish
    /// to override what was initially stored there, which would be the value calculated
    /// with `validate_checksum`.
    pub fn get_state(&self, position: u64, initialization_vector: Vec<u8>) -> JLAPState {
        JLAPState {
            position,
            initialization_vector,
            footer: self.footer.clone(),
        }
    }

    /// Validates the checksum present on a [`JLAPResponse`] struct
    pub fn validate_checksum(&self) -> Result<Option<Vec<u8>>, JLAPError> {
        let mut initialization_vector: Option<Output<Blake2b256>> = None;
        let mut iv_values: Vec<Output<Blake2bMac256>> = vec![];
        let end = self.lines.len() - 1;

        for line in &self.lines[self.offset..end] {
            initialization_vector = Some(blake2b_256_hash_with_key(
                line.as_bytes(),
                initialization_vector
                    .as_deref()
                    .unwrap_or(&self.initialization_vector),
            ));
            iv_values.push(initialization_vector.unwrap());
        }

        if let Some(validation_iv) = iv_values.pop() {
            if validation_iv != self.checksum {
                tracing::debug!(
                    "Checksum mismatch: {:x} != {:x}",
                    validation_iv,
                    self.checksum
                );
                return Err(JLAPError::ChecksumMismatch);
            }
            // The new initialization vector for the next JLAP request is the second to last
            // hash value in `iv_values` (i.e. the last "patch" line). When a request has no
            // new patches, we return None to indicate that the cache should not change its
            // initialization vector.
            if let Some(new_iv) = iv_values.pop() {
                Ok(Some(new_iv.to_vec()))
            } else {
                Ok(None)
            }
        } else {
            Err(JLAPError::NoHashFound)
        }
    }
}

/// Calculates the bytes offset. We default to zero if we receive a shorter than
/// expected vector.
fn get_bytes_offset(lines: &[&str]) -> u64 {
    if lines.len() >= JLAP_FOOTER_OFFSET {
        lines[0..lines.len() - JLAP_FOOTER_OFFSET]
            .iter()
            .map(|x| format!("{x}\n").into_bytes().len() as u64)
            .sum()
    } else {
        0
    }
}

/// Attempts to patch a current `repodata.json` file
///
/// This method first makes a request to fetch JLAP data we need. It relies on the information we
/// pass via the `repo_data_state` argument to retrieve the correct response.
///
/// After this, it will apply JSON patches to the file located at `repo_data_json_path`.
/// At the end, we compare the new `blake2b` hash with what was listed in the JLAP metadata to
/// ensure the file is correct.
///
/// The return value is the updated [`JLAPState`] and the Blake2b256 hash of the new file.
pub async fn patch_repo_data(
    client: &ClientWithMiddleware,
    subdir_url: Url,
    repo_data_state: RepoDataState,
    repo_data_json_path: &Path,
    reporter: Option<Arc<dyn Reporter>>,
) -> Result<(JLAPState, Blake2b256Hash), JLAPError> {
    // Determine what we should use as our starting state
    let mut jlap_state = get_jlap_state(repo_data_state.jlap);

    let jlap_url = subdir_url
        .join(JLAP_FILE_NAME)
        .expect("Valid URLs should always be join-able with this constant value");

    let download_report = reporter
        .as_deref()
        .map(|reporter| (reporter, reporter.on_download_start(&jlap_url)));
    let (response, position) =
        fetch_jlap_with_retry(&jlap_url, client, jlap_state.position).await?;
    let jlap_response_url = response.url().clone();
    let response_text = match response.text_with_progress(download_report).await {
        Ok(value) => value,
        Err(error) => return Err(error.into()),
    };
    if let Some((reporter, index)) = download_report {
        reporter.on_download_complete(&jlap_response_url, index);
    }

    // Update position as it may have changed
    jlap_state.position = position;

    let jlap = JLAPResponse::new(&response_text, &jlap_state)?;
    let hash = repo_data_state.blake2_hash_nominal.unwrap_or_default();
    let latest_hash = jlap.footer.latest;
    let new_iv = jlap
        .validate_checksum()?
        .unwrap_or(jlap_state.initialization_vector);

    // We already have the latest version; return early because there's nothing to do
    if latest_hash == hash {
        tracing::info!("The latest hash matches our local data. File up to date.");
        return Ok((
            jlap.get_state(jlap.new_position, new_iv),
            repo_data_state.blake2_hash.unwrap_or_default(),
        ));
    }

    // Applies patches and returns early if an error is encountered
    let hash = jlap.apply(repo_data_json_path, hash, reporter).await?;

    // Patches were applied successfully, so we need to update the position
    Ok((jlap.get_state(jlap.new_position, new_iv), hash))
}

/// Fetches a JLAP response from server
async fn fetch_jlap(
    url: &Url,
    client: &ClientWithMiddleware,
    range: &str,
) -> reqwest_middleware::Result<Response> {
    let request_builder = client.get(url.clone());
    let mut headers = HeaderMap::default();

    headers.insert(
        reqwest::header::RANGE,
        HeaderValue::from_str(range).unwrap(),
    );

    request_builder.headers(headers).send().await
}

/// Fetches the JLAP response but also retries in the case of a `RANGE_NOT_SATISFIABLE` error
///
/// When a JLAP file is updated on the server, it may cause new requests to trigger a
/// `RANGE_NOT_SATISFIABLE` error because the local cache is now out of sync. In this case, we
/// try the request once more from the beginning.
///
/// We return a new value for position if this was triggered so that we can update the
/// `JLAPState` accordingly.
async fn fetch_jlap_with_retry(
    url: &Url,
    client: &ClientWithMiddleware,
    position: u64,
) -> Result<(Response, u64), JLAPError> {
    tracing::info!("fetching JLAP state from {url} (bytes={position}-)");
    let range = format!("bytes={position}-");

    match fetch_jlap(url, client, &range).await {
        Ok(response) => {
            if response.status() == StatusCode::RANGE_NOT_SATISFIABLE && position != 0 {
                tracing::warn!(
                    "JLAP range request could not be satisfied, fetching the entire file.."
                );
                let range = "bytes=0-";
                return match fetch_jlap(url, client, range).await {
                    Ok(response) => Ok((response, 0)),
                    Err(error) => Err(error.into()),
                };
            }
            Ok((response, position))
        }
        Err(error) => Err(error.into()),
    }
}

/// Applies JLAP patches to a `repodata.json` file
///
/// This is a multi-step process that involves:
///
/// 1. Opening and parsing the current repodata file
/// 2. Applying patches to this repodata file
/// 3. Re-ordering the repo data
/// 4. Saving this repodata file to disk
fn apply_jlap_patches(
    patches: Arc<[Patch]>,
    start_index: usize,
    repo_data_path: &Path,
    reporter: Option<Arc<dyn Reporter>>,
) -> Result<Blake2b256Hash, JLAPError> {
    let report = reporter
        .as_deref()
        .map(|reporter| (reporter, reporter.on_jlap_start()));

    if let Some((reporter, index)) = report {
        reporter.on_jlap_decode_start(index);
    }

    // Read the contents of the current repodata to a string
    let repo_data_contents = fs::read_to_string(repo_data_path).map_err(JLAPError::FileSystem)?;

    // Parse the JSON so we can manipulate it
    tracing::info!("parsing cached repodata.json as JSON");
    let mut repo_data =
        serde_json::from_str::<Value>(&repo_data_contents).map_err(JLAPError::JSONParse)?;
    std::mem::drop(repo_data_contents);

    if let Some((reporter, index)) = report {
        reporter.on_jlap_decode_completed(index);
    }

    // Apply any patches that we have not already applied
    tracing::info!(
        "applying patches #{} through #{}",
        start_index + 1,
        patches.len()
    );
    for (patch_index, patch) in patches[start_index..].iter().enumerate() {
        if let Some((reporter, index)) = report {
            reporter.on_jlap_apply_patch(index, patch_index, patches.len());
        }
        if let Err(error) = json_patch::patch_unsafe(&mut repo_data, &patch.patch) {
            return Err(JLAPError::JSONPatch(error));
        }
    }

    if let Some((reporter, index)) = report {
        reporter.on_jlap_apply_patches_completed(index);
        reporter.on_jlap_encode_start(index);
    }

    // Write the content to disk and immediately compute the hash of the file contents.
    tracing::info!("writing patched repodata to disk");
    let mut hashing_writer = NamedTempFile::new_in(
        repo_data_path
            .parent()
            .expect("the repodata.json file must reside in a directory"),
    )
    .map_err(JLAPError::FileSystem)
    .map(rattler_digest::HashingWriter::<_, Blake2b256>::new)?;
    serde_json::to_writer(std::io::BufWriter::new(&mut hashing_writer), &repo_data)
        .map_err(JLAPError::JSONParse)?;

    let (file, hash) = hashing_writer.finalize();
    file.persist(repo_data_path)
        .map_err(|e| JLAPError::FileSystem(e.error))?;

    if let Some((reporter, index)) = report {
        reporter.on_jlap_encode_completed(index);
        reporter.on_jlap_completed(index);
    }

    Ok(hash)
}

/// Retrieves the correct values for `position` and `initialization_vector` from a `JLAPState` object
///
/// If we cannot find the correct values, we provide defaults from this module.
/// When we can correctly parse a hex string (the `initialization_vector` should always be a
/// hex string), we return an error.
fn get_jlap_state(state: Option<JLAPState>) -> JLAPState {
    match state {
        Some(state) => JLAPState {
            position: state.position,
            initialization_vector: state.initialization_vector,
            footer: state.footer,
        },
        None => JLAPState {
            position: JLAP_START_POSITION,
            initialization_vector: JLAP_START_INITIALIZATION_VECTOR.to_vec(),
            footer: JLAPFooter::default(),
        },
    }
}

/// Creates a keyed hash
fn blake2b_256_hash_with_key(data: &[u8], key: &[u8]) -> Output<Blake2bMac256> {
    let mut state = Blake2bMac256::new_with_salt_and_personal(key, &[], &[]).unwrap();
    state.update(data);
    state.finalize_fixed()
}

#[cfg(test)]
mod test {
    use super::patch_repo_data;
    use std::path::PathBuf;

    use crate::fetch::cache::RepoDataState;
    use crate::utils::simple_channel_server::SimpleChannelServer;

    use rattler_digest::{parse_digest_from_hex, Blake2b256};
    use reqwest_middleware::ClientWithMiddleware;
    use rstest::rstest;
    use tempfile::TempDir;
    use url::Url;

    use fs_err::tokio as tokio_fs;

    const FAKE_STATE_DATA_INITIAL: &str = r#"{
  "url": "https://repo.example.com/pkgs/main/osx-64/repodata.json.zst",
  "etag": "W/\"49aa6d9ea6f3285efe657780a7c8cd58\"",
  "mod": "Tue, 30 May 2023 20:03:48 GMT",
  "cache_control": "public, max-age=30",
  "mtime_ns": 1685509481332236078,
  "size": 38317593,
  "blake2_hash_nominal": "580100cb35459305eaaa31feeebacb06aad6422257754226d832e504666fc1c6",
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
  }
}"#;

    const FAKE_STATE_DATA_UPDATE_ONE: &str = r#"{
  "url": "https://repo.example.com/pkgs/main/osx-64/repodata.json.zst",
  "etag": "W/\"49aa6d9ea6f3285efe657780a7c8cd58\"",
  "mod": "Tue, 30 May 2023 20:03:48 GMT",
  "cache_control": "public, max-age=30",
  "mtime_ns": 1685509481332236078,
  "size": 38317593,
  "blake2_hash_nominal": "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9",
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
    "iv": "5ec4a4fc3afd07b398ed78ffbd30ce3ef7c1f935f0e0caffc61455352ceedeff",
    "pos": 738,
    "footer": {
      "url": "repodata.json",
      "latest": "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9"
    }
  }
}"#;

    const FAKE_STATE_DATA_UPDATE_TWO: &str = r#"{
  "url": "https://repo.example.com/pkgs/main/osx-64/repodata.json.zst",
  "etag": "W/\"49aa6d9ea6f3285efe657780a7c8cd58\"",
  "mod": "Tue, 30 May 2023 20:03:48 GMT",
  "cache_control": "public, max-age=30",
  "mtime_ns": 1685509481332236078,
  "size": 38317593,
  "blake2_hash_nominal": "160b529c5f72b9755f951c1b282705d49d319a5f2f80b33fb1a670d02ddeacf9",
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
    "iv": "7d6e2b5185cf5e14f852355dc79eeba1233550d974f274f1eaf7db21c7b2c4e8",
    "pos": 1341,
    "footer": {
      "url": "repodata.json",
      "latest": "160b529c5f72b9755f951c1b282705d49d319a5f2f80b33fb1a670d02ddeacf9"
    }
  }
}"#;

    const FAKE_STATE_DATA_OUT_OF_BOUNDS_POSITION: &str = r#"{
  "url": "https://repo.example.com/pkgs/main/osx-64/repodata.json.zst",
  "etag": "W/\"49aa6d9ea6f3285efe657780a7c8cd58\"",
  "mod": "Tue, 30 May 2023 20:03:48 GMT",
  "cache_control": "public, max-age=30",
  "mtime_ns": 1685509481332236078,
  "size": 38317593,
  "blake2_hash_nominal": "160b529c5f72b9755f951c1b282705d49d319a5f2f80b33fb1a670d02ddeacf9",
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
    "iv": "7d6e2b5185cf5e14f852355dc79eeba1233550d974f274f1eaf7db21c7b2c4e8",
    "pos": 9999,
    "footer": {
      "url": "repodata.json",
      "latest": "160b529c5f72b9755f951c1b282705d49d319a5f2f80b33fb1a670d02ddeacf9"
    }
  }
}"#;

    const FAKE_REPO_DATA_INITIAL: &str = r#"{
  "info": {
    "subdir": "osx-64"
  },
  "packages": {},
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
  "removed": [],
  "repodata_version": 1
}"#;

    const FAKE_REPO_DATA_UPDATE_ONE: &str = "{\"info\":{\"subdir\":\"osx-64\"},\"packages\":{},\"packages.conda\":{\"zstd-1.5.4-hc035e20_0.conda\":{\"build\":\"hc035e20_0\",\"build_number\":0,\"depends\":[\"libcxx >=14.0.6\",\"lz4-c >=1.9.4,<1.10.0a0\",\"xz >=5.2.10,<6.0a0\",\"zlib >=1.2.13,<1.3.0a0\"],\"license\":\"BSD-3-Clause AND GPL-2.0-or-later\",\"license_family\":\"BSD\",\"md5\":\"f284fea068c51b1a0eaea3ac58c300c0\",\"name\":\"zstd\",\"sha256\":\"0af4513ef7ad7fa8854fa714130c25079f3744471fc106f47df80eb10c34429d\",\"size\":605550,\"subdir\":\"osx-64\",\"timestamp\":1680034665911,\"version\":\"1.5.4\"},\"zstd-1.5.5-hc035e20_0.conda\":{\"build\":\"hc035e20_0\",\"build_number\":0,\"depends\":[\"libcxx >=14.0.6\",\"lz4-c >=1.9.4,<1.10.0a0\",\"xz >=5.2.10,<6.0a0\",\"zlib >=1.2.13,<1.3.0a0\"],\"license\":\"BSD-3-Clause AND GPL-2.0-or-later\",\"license_family\":\"BSD\",\"md5\":\"5e0b7ddb1b7dc6b630e1f9a03499c19c\",\"name\":\"zstd\",\"sha256\":\"5b192501744907b841de036bb89f5a2776b4cac5795ccc25dcaebeac784db038\",\"size\":622467,\"subdir\":\"osx-64\",\"timestamp\":1681304595869,\"version\":\"1.5.5\"}},\"removed\":[],\"repodata_version\":1}";

    const FAKE_REPO_DATA_UPDATE_TWO: &str = "{\"info\":{\"subdir\":\"osx-64\"},\"packages\":{},\"packages.conda\":{\"zstd-1.5.4-hc035e20_0.conda\":{\"build\":\"hc035e20_0\",\"build_number\":0,\"depends\":[\"libcxx >=14.0.6\",\"lz4-c >=1.9.4,<1.10.0a0\",\"xz >=5.2.10,<6.0a0\",\"zlib >=1.2.13,<1.3.0a0\"],\"license\":\"BSD-3-Clause AND GPL-2.0-or-later\",\"license_family\":\"BSD\",\"md5\":\"f284fea068c51b1a0eaea3ac58c300c0\",\"name\":\"zstd\",\"sha256\":\"0af4513ef7ad7fa8854fa714130c25079f3744471fc106f47df80eb10c34429d\",\"size\":605550,\"subdir\":\"osx-64\",\"timestamp\":1680034665911,\"version\":\"1.5.4\"},\"zstd-1.5.5-hc035e20_0.conda\":{\"build\":\"hc035e20_0\",\"build_number\":0,\"depends\":[\"libcxx >=14.0.6\",\"lz4-c >=1.9.4,<1.10.0a0\",\"xz >=5.2.10,<6.0a0\",\"zlib >=1.2.13,<1.3.0a0\"],\"license\":\"BSD-3-Clause AND GPL-2.0-or-later\",\"license_family\":\"BSD\",\"md5\":\"5e0b7ddb1b7dc6b630e1f9a03499c19c\",\"name\":\"zstd\",\"sha256\":\"5b192501744907b841de036bb89f5a2776b4cac5795ccc25dcaebeac784db038\",\"size\":622467,\"subdir\":\"osx-64\",\"timestamp\":1681304595869,\"version\":\"1.5.5\"},\"zstd-static-1.4.5-hb1e8313_0.conda\":{\"build\":\"hb1e8313_0\",\"build_number\":0,\"depends\":[\"libcxx >=10.0.0\",\"zstd 1.4.5 h41d2c2f_0\"],\"license\":\"BSD 3-Clause\",\"md5\":\"5447986040e0b73d6c681a4d8f615d6c\",\"name\":\"zstd-static\",\"sha256\":\"3759ab53ff8320d35c6db00d34059ba99058eeec1cbdd0da968c5e12f73f7658\",\"size\":13930,\"subdir\":\"osx-64\",\"timestamp\":1595965109852,\"version\":\"1.4.5\"}},\"removed\":[],\"repodata_version\":1}";

    const FAKE_REPO_DATA_UPDATE_ONE_HASH: &str =
        "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9";

    const FAKE_REPO_DATA_UPDATE_TWO_HASH: &str =
        "160b529c5f72b9755f951c1b282705d49d319a5f2f80b33fb1a670d02ddeacf9";

    const FAKE_JLAP_DATA_INITIAL: &str = r#"0000000000000000000000000000000000000000000000000000000000000000
{"to": "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9", "from": "580100cb35459305eaaa31feeebacb06aad6422257754226d832e504666fc1c6", "patch": [{"op": "add", "path": "/packages.conda/zstd-1.5.5-hc035e20_0.conda", "value": {"build": "hc035e20_0","build_number": 0,"depends": ["libcxx >=14.0.6","lz4-c >=1.9.4,<1.10.0a0","xz >=5.2.10,<6.0a0","zlib >=1.2.13,<1.3.0a0"],"license": "BSD-3-Clause AND GPL-2.0-or-later","license_family": "BSD","md5": "5e0b7ddb1b7dc6b630e1f9a03499c19c","name": "zstd","sha256": "5b192501744907b841de036bb89f5a2776b4cac5795ccc25dcaebeac784db038","size": 622467,"subdir": "osx-64","timestamp": 1681304595869, "version": "1.5.5"}}]}
{"url": "repodata.json", "latest": "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9"}
5cf5bb373f361fe30d41891399d148f9c9dd0cc5f381e64f8fa3e7febd7269f0"#;

    const FAKE_JLAP_DATA_UPDATE_ONE: &str = r#"0000000000000000000000000000000000000000000000000000000000000000
{"to": "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9", "from": "580100cb35459305eaaa31feeebacb06aad6422257754226d832e504666fc1c6", "patch": [{"op": "add", "path": "/packages.conda/zstd-1.5.5-hc035e20_0.conda", "value": {"build": "hc035e20_0","build_number": 0,"depends": ["libcxx >=14.0.6","lz4-c >=1.9.4,<1.10.0a0","xz >=5.2.10,<6.0a0","zlib >=1.2.13,<1.3.0a0"],"license": "BSD-3-Clause AND GPL-2.0-or-later","license_family": "BSD","md5": "5e0b7ddb1b7dc6b630e1f9a03499c19c","name": "zstd","sha256": "5b192501744907b841de036bb89f5a2776b4cac5795ccc25dcaebeac784db038","size": 622467,"subdir": "osx-64","timestamp": 1681304595869, "version": "1.5.5"}}]}
{"to": "160b529c5f72b9755f951c1b282705d49d319a5f2f80b33fb1a670d02ddeacf9", "from": "9b76165ba998f77b2f50342006192bf28817dad474d78d760ab12cc0260e3ed9", "patch": [{"op": "add", "path": "/packages.conda/zstd-static-1.4.5-hb1e8313_0.conda", "value": {"build": "hb1e8313_0", "build_number": 0, "depends": ["libcxx >=10.0.0", "zstd 1.4.5 h41d2c2f_0"], "license": "BSD 3-Clause", "md5": "5447986040e0b73d6c681a4d8f615d6c", "name": "zstd-static", "sha256": "3759ab53ff8320d35c6db00d34059ba99058eeec1cbdd0da968c5e12f73f7658", "size": 13930, "subdir": "osx-64", "timestamp": 1595965109852, "version": "1.4.5"}}]}
{"url": "repodata.json", "latest": "160b529c5f72b9755f951c1b282705d49d319a5f2f80b33fb1a670d02ddeacf9"}
5a4c42192a69299198bd8cfc85146d725d0dcc24a4e50f6eab383bc37cab2d2d"#;

    /// Provides all the necessary environment setup for a test
    struct TestEnvironment {
        /// These fields are never read but must stay in scope for tests succeed
        _server: SimpleChannelServer,
        _subdir_path: TempDir,
        _cache_dir: TempDir,

        pub server_url: Url,

        pub cache_repo_data: PathBuf,

        pub client: ClientWithMiddleware,

        pub repo_data_state: RepoDataState,
    }

    impl TestEnvironment {
        pub async fn new(repo_data: &str, jlap_data: &str, repo_data_state: &str) -> Self {
            let subdir_path = setup_server_environment(None, Some(jlap_data)).await;
            let server = SimpleChannelServer::new(subdir_path.path()).await;
            let server_url = server.url();

            let (cache_dir, cache_repo_data) =
                setup_client_environment(&server_url, Some(repo_data)).await;

            let client = reqwest::Client::new().into();

            let repo_data_state: RepoDataState = serde_json::from_str(repo_data_state).unwrap();

            Self {
                _server: server,
                _subdir_path: subdir_path,
                _cache_dir: cache_dir,
                server_url,
                cache_repo_data,
                client,
                repo_data_state,
            }
        }
    }

    /// Writes the desired files to the "server" environment
    async fn setup_server_environment(
        server_repo_data: Option<&str>,
        server_jlap: Option<&str>,
    ) -> TempDir {
        // Create a directory with some repodata; this is the "server" data
        let subdir_path = TempDir::new().unwrap();

        if let Some(content) = server_jlap {
            // Add files we need to request to the server
            tokio_fs::write(subdir_path.path().join("repodata.jlap"), content)
                .await
                .unwrap();
        }

        if let Some(content) = server_repo_data {
            // Add files we need to request to the server
            tokio_fs::write(subdir_path.path().join("repodata.json"), content)
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
        let cache_repo_data_path = cache_dir.path().join(format!("{cache_key}.json"));

        if let Some(content) = cache_repo_data {
            tokio::fs::write(cache_repo_data_path.clone(), content)
                .await
                .unwrap();
        }

        (cache_dir, cache_repo_data_path)
    }

    #[rstest]
    #[case::patch_repo_data_with_no_previous_jlap_cache_state(
        FAKE_REPO_DATA_INITIAL,
        FAKE_REPO_DATA_UPDATE_ONE,
        FAKE_JLAP_DATA_INITIAL,
        FAKE_STATE_DATA_INITIAL,
        738,
        "5ec4a4fc3afd07b398ed78ffbd30ce3ef7c1f935f0e0caffc61455352ceedeff",
        FAKE_REPO_DATA_UPDATE_ONE_HASH
    )]
    #[case::patch_repo_data_with_a_partial_jlap_response_and_a_previous_jlap_cache_state(
        FAKE_REPO_DATA_UPDATE_ONE,
        FAKE_REPO_DATA_UPDATE_TWO,
        FAKE_JLAP_DATA_UPDATE_ONE,
        FAKE_STATE_DATA_UPDATE_ONE,
        1341,
        "7d6e2b5185cf5e14f852355dc79eeba1233550d974f274f1eaf7db21c7b2c4e8",
        FAKE_REPO_DATA_UPDATE_TWO_HASH
    )]
    #[case::patch_repo_data_with_no_new_patches_to_apply(
        FAKE_REPO_DATA_UPDATE_TWO,
        FAKE_REPO_DATA_UPDATE_TWO,
        FAKE_JLAP_DATA_UPDATE_ONE,
        FAKE_STATE_DATA_UPDATE_TWO,
        1341,
        "7d6e2b5185cf5e14f852355dc79eeba1233550d974f274f1eaf7db21c7b2c4e8",
        FAKE_REPO_DATA_UPDATE_TWO_HASH
    )]
    #[case::patch_repo_data_trigger_range_not_satisfiable_recovery_workflow(
        FAKE_REPO_DATA_UPDATE_TWO,
        FAKE_REPO_DATA_UPDATE_TWO,
        FAKE_JLAP_DATA_UPDATE_ONE,
        FAKE_STATE_DATA_OUT_OF_BOUNDS_POSITION,
        1341,
        "7d6e2b5185cf5e14f852355dc79eeba1233550d974f274f1eaf7db21c7b2c4e8",
        FAKE_REPO_DATA_UPDATE_TWO_HASH
    )]
    #[tokio::test]
    /// This is the primary test for this module. Using the parameters above, it covers a variety
    /// of use cases. Check out the parameter descriptions from more information about what the
    /// test is trying to do.
    pub async fn test_patch_repo_data(
        #[case] repo_data: &str,
        #[case] expected_repo_data: &str,
        #[case] jlap_data: &str,
        #[case] repo_data_state: &str,
        #[case] expected_position: u64,
        #[case] expected_initialization_vector: &str,
        #[case] expected_hash: &str,
    ) {
        let test_env = TestEnvironment::new(repo_data, jlap_data, repo_data_state).await;

        let (updated_jlap_state, _hash) = patch_repo_data(
            &test_env.client,
            test_env.server_url,
            test_env.repo_data_state,
            &test_env.cache_repo_data,
            None,
        )
        .await
        .unwrap();

        // Make assertions
        let repo_data = tokio::fs::read_to_string(test_env.cache_repo_data)
            .await
            .unwrap();

        // Ensure the repo data was updated appropriately
        assert_eq!(repo_data, expected_repo_data);

        // Ensure the the updated JLAP state matches what it should
        assert_eq!(updated_jlap_state.position, expected_position);
        assert_eq!(
            hex::encode(updated_jlap_state.initialization_vector),
            expected_initialization_vector
        );
        assert_eq!(
            updated_jlap_state.footer.latest,
            parse_digest_from_hex::<Blake2b256>(expected_hash).unwrap()
        );
    }
}
