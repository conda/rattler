//! `rattler-conda-types` contains data models for types commonly found within the Conda ecosystem.
//! The library itself doesnt provide any functionality besides parsing the data types.

mod channel;
mod channel_data;
mod explicit_environment_spec;
mod match_spec;
mod no_arch_type;
mod platform;
mod repo_data;
mod run_export;
mod utils;
mod version;
mod version_spec;

pub mod package;

pub use channel::{Channel, ChannelConfig, ParseChannelError};
pub use channel_data::{ChannelData, ChannelDataPackage};
pub use explicit_environment_spec::{
    ExplicitEnvironmentEntry, ExplicitEnvironmentSpec, PackageArchiveHash,
    ParseExplicitEnvironmentSpecError, ParsePackageArchiveHashError,
};
pub use match_spec::MatchSpec;
pub use no_arch_type::{NoArchKind, NoArchType};
pub use platform::{ParsePlatformError, Platform};
pub use repo_data::{PackageRecord, RepoData};
pub use run_export::{RunExportKind, RunExports};
pub use version::{ParseVersionError, ParseVersionErrorKind, Version};
pub use version_spec::VersionSpec;

#[cfg(test)]
use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) fn get_test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
}
