#![deny(missing_docs)]
//! `rattler-conda-types` contains data models for types commonly found within
//! the Conda ecosystem. The library itself doesnt provide any functionality
//! besides parsing the data types.

mod build_spec;
mod channel;
mod channel_data;
mod explicit_environment_spec;
mod match_spec;
mod no_arch_type;
mod parse_mode;
mod platform;
mod repo_data;
mod repo_data_record;
mod run_export;
mod utils;
mod version;
pub mod version_spec;

mod environment_yaml;
mod generic_virtual_package;
pub mod package;
mod package_name;
pub mod prefix_record;

#[cfg(test)]
use std::path::{Path, PathBuf};

pub use build_spec::{BuildNumber, BuildNumberSpec, ParseBuildNumberSpecError};
pub use channel::{Channel, ChannelConfig, NamedChannelOrUrl, ParseChannelError};
pub use channel_data::{ChannelData, ChannelDataPackage};
pub use environment_yaml::{EnvironmentYaml, MatchSpecOrSubSection};
pub use explicit_environment_spec::{
    ExplicitEnvironmentEntry, ExplicitEnvironmentSpec, PackageArchiveHash,
    ParseExplicitEnvironmentSpecError, ParsePackageArchiveHashError,
};
pub use generic_virtual_package::GenericVirtualPackage;
pub use match_spec::{
    matcher::{StringMatcher, StringMatcherParseError},
    parse::ParseMatchSpecError,
    MatchSpec, Matches, NamelessMatchSpec,
};
pub use no_arch_type::{NoArchKind, NoArchType};
pub use package_name::{InvalidPackageNameError, PackageName};
pub use parse_mode::ParseStrictness;
pub use platform::{Arch, ParseArchError, ParsePlatformError, Platform};
pub use prefix_record::PrefixRecord;
pub use repo_data::{
    compute_package_url,
    patches::{PackageRecordPatch, PatchInstructions, RepoDataPatch},
    sharded::{Shard, ShardedRepodata, ShardedSubdirInfo},
    ChannelInfo, ConvertSubdirError, PackageRecord, RepoData, ValidatePackageRecordsError,
};
pub use repo_data_record::RepoDataRecord;
pub use run_export::RunExportKind;
pub use version::{
    Component, ParseVersionError, ParseVersionErrorKind, StrictVersion, Version, VersionBumpError,
    VersionBumpType, VersionExtendError, VersionWithSource,
};
pub use version_spec::VersionSpec;

/// An package identifier that can be used to identify packages across package
/// ecosystems.
pub type PackageUrl = purl::GenericPurl<String>;

#[cfg(test)]
pub(crate) fn get_test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
}
