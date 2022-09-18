//! Rattler is an experimental library and executable to work with [Conda](http://conda.io)
//! environments. Conda is a cross-platform open-source package management system and environment
//! management system.
//!
//! Conda is originally written in Python and has evolved a lot since it was first conceived.
//! Rattler is an attempt at reimplementing a lot of the machinery supporting Conda but making it
//! available to a wider range of languages. The goal is to be able to integrate the Conda ecosystem
//! in a wide variaty of tools that do not rely on Python. Rust has excellent support for
//! interfacing with many other languages (WASM, Javascript, Python, C, etc) and is therefor a good
//! candidate for a reimplementation.

mod channel;
mod channel_data;
mod environment;
mod match_spec;
mod no_arch_type;
mod package_archive;
mod package_cache;
mod platform;
pub mod repo_data;
mod run_export;
pub mod solver;
pub(crate) mod utils;
mod version;
mod version_spec;

pub use channel::{Channel, ChannelConfig, ParseChannelError};
pub use channel_data::{ChannelData, ChannelDataPackage};
pub use environment::Environment;
pub use match_spec::MatchSpec;
pub use no_arch_type::{NoArchKind, NoArchType};
pub use package_archive::PackageArchiveFormat;
pub use platform::{ParsePlatformError, Platform};
pub use repo_data::{PackageRecord, RepoData};
pub use run_export::{RunExportKind, RunExports};
pub use version::{ParseVersionError, ParseVersionErrorKind, Version};
pub use version_spec::VersionSpec;

/// A helper function that returns a [`Channel`] instance that points to an empty channel on disk
/// that is bundled with this repository.
#[cfg(any(doctest, test))]
pub fn empty_channel() -> Channel {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let channel_path = manifest_dir.join("resources/channels/empty");
    Channel::from_str(
        &format!("file://{}[noarch]", channel_path.display()),
        &ChannelConfig::default(),
    )
    .unwrap()
}
