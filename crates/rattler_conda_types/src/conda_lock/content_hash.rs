use crate::conda_lock::Channel;
use crate::{MatchSpec, Platform};
use serde::Serialize;
use serde_json_python_formatter::PythonFormatter;
use std::string::FromUtf8Error;

#[derive(Debug, thiserror::Error)]
pub enum CalculateContentHashError {
    #[error("the data for key `{0}` is required but missing")]
    RequiredAttributeMissing(String),
    #[error(transparent)]
    JsonDecodeError(#[from] serde_json::Error),
    #[error(transparent)]
    Utf8Error(#[from] FromUtf8Error),
}

/// This function tries to replicate the creation of the content-hashes
/// like conda-lock does https://github.com/conda/conda-lock/blob/83117cb8da89d011a25f643f953822d5c098b246/conda_lock/models/lock_spec.py#L60
/// so we need to recreate some python data-structures and serialize these to json
pub fn calculate_content_data(
    _platform: &Platform,
    input_specs: &[MatchSpec],
    channels: &[Channel],
) -> Result<String, CalculateContentHashError> {
    /// Selector taken from the conda-lock python source code
    /// which we will just keep empty for now
    #[derive(Serialize, Default, Debug)]
    struct Selector {
        platform: Option<Vec<String>>,
    }

    /// This is the equivalent of an VersionedDependency from
    /// the conda-lock python source code
    /// conda
    #[derive(Serialize, Debug)]
    struct CondaLockVersionedDependency {
        build: Option<String>,
        category: String,
        conda_channel: Option<String>,
        extras: Vec<String>,
        manager: String,
        name: String,
        optional: bool,
        selectors: Selector,
        version: String,
    }

    /// Data for which the ContentHash hash has to be constructed
    /// In python this is just a dictionary
    #[derive(Serialize, Debug)]
    struct ContentHashData {
        channels: Vec<Channel>,
        specs: Vec<CondaLockVersionedDependency>,
    }

    // Map our stuff to conda-lock types
    let specs = input_specs
        .iter()
        .map(|spec| {
            Ok(CondaLockVersionedDependency {
                name: spec
                    .name
                    .clone()
                    .ok_or_else(|| {
                        CalculateContentHashError::RequiredAttributeMissing("name".to_string())
                    })?
                    .as_source()
                    .to_string(),
                manager: "conda".to_string(),
                optional: false,
                category: "main".to_string(),
                extras: Default::default(),
                selectors: Default::default(),
                version: spec
                    .version
                    .as_ref()
                    .map(|v| v.to_string())
                    .ok_or_else(|| {
                        CalculateContentHashError::RequiredAttributeMissing("version".to_string())
                    })?,
                build: spec.build.clone().map(|b| match b {
                    crate::StringMatcher::Exact(s) => s,
                    crate::StringMatcher::Glob(g) => format!("{}", g),
                    crate::StringMatcher::Regex(r) => format!("{}", r),
                }),
                conda_channel: None,
            })
        })
        .collect::<Result<Vec<_>, CalculateContentHashError>>()?;

    // In the python code they are also adding a virtual package hash
    // For virtual packages overwritten by the user, we are skipping
    // this for now
    // TODO: Add default list of virtual packages and then create the content hashing

    // Create the python dict
    let content_hash_data = ContentHashData {
        channels: channels.to_vec(),
        specs,
    };

    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, PythonFormatter::default());
    content_hash_data.serialize(&mut ser)?;
    Ok(String::from_utf8(buf)?)
}

/// Calculate the content hash for a platform and set of match-specs
pub fn calculate_content_hash(
    platform: &Platform,
    input_specs: &[MatchSpec],
    channels: &[Channel],
) -> Result<String, CalculateContentHashError> {
    let content_data = calculate_content_data(platform, input_specs, channels)?;
    let content_hash =
        rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(&content_data);
    Ok(format!("{:x}", content_hash))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use crate::conda_lock::content_hash;
    use crate::{MatchSpec, Platform};

    #[test]
    fn test_content_data() {
        let output = content_hash::calculate_content_data(
            &Platform::Osx64,
            &[MatchSpec::from_str("python =3.11.0").unwrap()],
            &["conda-forge".into()],
        );

        // This is output taken from running the conda-lock code
        // we compare the
        let str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test-data/conda-lock/content_hash/python.txt"
        ));
        assert_eq!(str, output.unwrap());

        // TODO: add actual hash output checking when we have a default virtual package list
        //assert_eq!()
    }

    #[test]
    fn test_content_hash() {
        let output = content_hash::calculate_content_hash(
            &Platform::Osx64,
            &[MatchSpec::from_str("python =3.11.0").unwrap()],
            &["conda-forge".into()],
        )
        .unwrap();

        assert_eq!(
            output,
            "66c2193c7a9f1172bbd93eaf49119bd478d1408da018b2944974bbc8d85a6a50"
        );
    }
}
