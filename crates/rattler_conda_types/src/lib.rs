#![deny(missing_docs)]
//! `rattler-conda-types` contains data models for types commonly found within the Conda ecosystem.
//! The library itself doesnt provide any functionality besides parsing the data types.

mod channel;
mod channel_data;
mod explicit_environment_spec;
mod match_spec;
mod no_arch_type;
mod platform;
mod repo_data;
mod repo_data_record;
mod run_export;
mod utils;
mod version;
pub mod version_spec;

pub mod conda_lock;
mod generic_virtual_package;
pub mod package;
pub mod prefix_record;

pub use channel::{Channel, ChannelConfig, ParseChannelError};
pub use channel_data::{ChannelData, ChannelDataPackage};
pub use explicit_environment_spec::{
    ExplicitEnvironmentEntry, ExplicitEnvironmentSpec, PackageArchiveHash,
    ParseExplicitEnvironmentSpecError, ParsePackageArchiveHashError,
};
pub use generic_virtual_package::GenericVirtualPackage;
pub use match_spec::matcher::StringMatcher;
pub use match_spec::parse::ParseMatchSpecError;
pub use match_spec::{MatchSpec, NamelessMatchSpec};
pub use no_arch_type::{NoArchKind, NoArchType};
pub use platform::{ParsePlatformError, Platform};
pub use prefix_record::PrefixRecord;
pub use repo_data::patches::{PackageRecordPatch, PatchInstructions, RepoDataPatch};
pub use repo_data::{ChannelInfo, ConvertSubdirError, PackageRecord, RepoData};
pub use repo_data_record::RepoDataRecord;
pub use run_export::RunExportKind;
pub use version::{ParseVersionError, ParseVersionErrorKind, Version, VersionWithSource};
pub use version_spec::VersionSpec;

#[cfg(test)]
use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) fn get_test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
}
