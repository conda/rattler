//! [![Rattler banner](https://github.com/user-attachments/assets/bfd64756-061d-49f5-af4e-388743bdb855)](https://github.com/conda/rattler)
//!
//! Rattler is a library and executable to work with [Conda](http://conda.io)
//! environments. Conda is a cross-platform open-source package management
//! system and environment management system.
//!
//! Conda is originally written in Python and has evolved a lot since it was
//! first conceived. Rattler is an attempt at reimplementing a lot of the
//! machinery supporting Conda but making it available to a wider range of
//! languages. The goal is to be able to integrate the Conda ecosystem in a wide
//! variety of tools that do not rely on Python. Rust has excellent support for
//! interfacing with many other languages (WASM, Javascript, Python, C, etc) and
//! is therefore a good candidate for a reimplementation.
#![deny(missing_docs)]

use std::path::PathBuf;

#[cfg(feature = "cli-tools")]
pub mod cli;
pub mod install;
pub use rattler_cache::{package_cache, validation};

/// A helper function that returns a [`Channel`] instance that points to an
/// empty channel on disk that is bundled with this repository.
#[cfg(any(doctest, test))]
pub fn empty_channel() -> rattler_conda_types::Channel {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let channel_path = manifest_dir.join("../../test-data/channels/empty");
    rattler_conda_types::Channel::from_str(
        format!("file://{}[noarch]", channel_path.display()),
        &rattler_conda_types::ChannelConfig::default_with_root_dir(
            std::env::current_dir().unwrap(),
        ),
    )
    .unwrap()
}

#[cfg(test)]
pub(crate) fn get_test_data_dir() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
}

/// Returns the default cache directory used by rattler.
pub fn default_cache_dir() -> anyhow::Result<PathBuf> {
    rattler_cache::default_cache_dir()
}

#[cfg(test)]
use rattler_conda_types::RepoDataRecord;

#[cfg(test)]
pub(crate) fn get_repodata_record(package_path: impl AsRef<std::path::Path>) -> RepoDataRecord {
    use std::fs;

    use rattler_conda_types::{package::IndexJson, PackageRecord};
    use rattler_digest::{Md5, Sha256};
    use rattler_package_streaming::seek::read_package_file;

    let package_path = package_path.as_ref();
    let index_json = read_package_file::<IndexJson>(&package_path).unwrap();

    // find size and hash
    let size = fs::metadata(package_path).unwrap().len();
    let sha256 = rattler_digest::compute_file_digest::<Sha256>(&package_path).unwrap();
    let md5 = rattler_digest::compute_file_digest::<Md5>(&package_path).unwrap();

    RepoDataRecord {
        package_record: PackageRecord::from_index_json(
            index_json,
            Some(size),
            Some(sha256),
            Some(md5),
        )
        .unwrap(),
        file_name: package_path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap()
            .to_string(),
        url: url::Url::from_file_path(package_path).unwrap(),
        channel: "test".to_string(),
    }
}
