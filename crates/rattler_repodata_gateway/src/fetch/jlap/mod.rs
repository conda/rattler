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

use reqwest::{Client};
use serde::{Serialize, Deserialize};

/// Represents the variety of errors that we come across while processing JLAP files
#[derive(Debug, thiserror::Error)]
pub enum JLAPError {
    #[error(transparent)]
    /// Pass-thru for JSON errors found while parsing JLAP file
    JSONParseError(serde_json::Error),

    #[error("No patches found in JLAP file")]
    /// Error returned when JLAP file has no patches in it
    NoPatchesFoundError
}

/// Represents the numerous patches found in a JLAP file which makes up a majority
/// of the file
#[derive(Serialize, Deserialize, Debug)]
pub struct Patch {
    to: String,
    from: String,
    patch: json_patch::Patch, // [] is a valid, empty patch
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

/// Fetches a JLAP object from server.
///
/// To do this, we first need information about the current JLAP file that we have
/// on disk if it is in fact there. If we have an existing JLAP file, we need to figure
/// out how many patches to fetch from the server.
///
pub async fn fetch_jlap (url: &str, client: &Client) -> Result<reqwest::Response, reqwest::Error> {
    let request_builder = client.get(url);

    // TODO: Build headers here; this is where the incremental retrieving magic happens...

    request_builder.send().await
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
/// To do this, we simply see which lines begin with "{", which would be the
/// opening of the JSON object. The last two lines of the string we receive
/// do not contain patches and we therefore skip them.
pub fn convert_jlap_string_to_patch_set (text: &str) -> Result<Vec<Patch>, JLAPError>  {
    let lines: Vec<&str> = text.split("\n").collect();
    let length = lines.len();
    let jlap_footer_offset: usize = 2; // Last two lines do not contain patches

    if length > 1 {
        let patch_lines = lines[..length - jlap_footer_offset].iter();
        let patches: Result<Vec<Patch>, JLAPError> = patch_lines.filter(
            |line| line.starts_with("{")
        ).map(
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

/// Converts the body of a JLAP request to a vector of hashes
pub fn convert_string_to_hashes (text: &str) -> Vec<&str> {
    text.split(
        "\n"
    ).filter(
        |line | !line.starts_with("{")
    ).collect()
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
            convert_jlap_string_to_patch_set(test_string),
            Err(JLAPError::NoPatchesFoundError)
        );

        let test_string = "bad_data\nbad_data";

        assert_matches!(
            convert_jlap_string_to_patch_set(test_string),
            Err(JLAPError::NoPatchesFoundError)
        );

        let test_string = "";

        assert_matches!(
            convert_jlap_string_to_patch_set(test_string),
            Err(JLAPError::NoPatchesFoundError)
        );
    }

    #[test]
    pub fn test_convert_jlap_string_to_patch_set_success() {
        let patches = convert_jlap_string_to_patch_set(FAKE_JLAP_DATA).unwrap();

        assert_eq!(patches.len(), 1);
    }
}