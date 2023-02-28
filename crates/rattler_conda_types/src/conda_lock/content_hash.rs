use crate::conda_lock::Channel;
use crate::{MatchSpec, Platform};
use rattler_digest::serde::SerializableHash;
use serde::Serialize;

/// This function tries to replicate the creation of the content-hashes
/// like conda-lock does https://github.com/conda/conda-lock/blob/83117cb8da89d011a25f643f953822d5c098b246/conda_lock/models/lock_spec.py#L60
/// so we need to recreate some python data-structures and serialize these to json
pub fn calculate_content_data(
    _platform: &Platform,
    input_specs: &[MatchSpec],
    channels: &[Channel],
) -> String {
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
        .map(|spec| CondaLockVersionedDependency {
            name: spec.name.clone().unwrap(),
            manager: "conda".to_string(),
            optional: false,
            category: "main".to_string(),
            extras: Default::default(),
            selectors: Default::default(),
            version: spec.version.as_ref().map(|v| v.to_string()).unwrap(),
            build: spec.build.clone(),
            conda_channel: None,
        })
        .collect();

    // In the python code they are also adding a virtual package hash
    // For virtual packages overwritten by the user, we are skipping
    // this for now
    // TODO: Add default list of virtual packages and then create the content hashing

    // Create the python dict
    let content_hash_data = ContentHashData {
        channels: channels.to_vec(),
        specs,
    };

    // Create the json
    serde_json::to_string(&content_hash_data)
        .unwrap()
        // Replace these because python encodes with spaces
        .replace(":", ": ")
        .replace(",", ", ")
}

/// Calculate the content hash for a platform and set of matchspecs
pub fn calculate_content_hash(
    _platform: &Platform,
    input_specs: &[MatchSpec],
    channels: &[Channel],
) -> String {
    serde_json::to_string(&SerializableHash::<sha2::Sha256>(
        rattler_digest::compute_bytes_digest::<sha2::Sha256>(calculate_content_data(
            _platform,
            input_specs,
            channels,
        )),
    ))
    .unwrap()
}

#[cfg(test)]
mod tests {
    use crate::conda_lock::content_hash;
    use crate::{ChannelConfig, MatchSpec, Platform};

    #[test]
    fn test_content_hash() {
        let channel_config = ChannelConfig::default();
        let output = content_hash::calculate_content_data(
            &Platform::Osx64,
            &[MatchSpec::from_str("python =3.11.0", &channel_config).unwrap()],
            &["conda-forge".into()],
        );

        // This is output taken from running the conda-lock code
        // we compare the
        let str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test-data/conda-lock/content_hash/python.txt"
        ));
        assert_eq!(str, output);

        // TODO: add actual hash output checking when we have a default virtual package list
        //assert_eq!()
    }
}
