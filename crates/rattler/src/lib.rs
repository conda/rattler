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
mod match_spec;
mod match_spec_constraints;
mod platform;
mod repo_data;
mod solver;
pub(crate) mod utils;
mod version;
mod version_spec;

pub use channel::{
    Channel, ChannelConfig, FetchRepoDataError, FetchRepoDataProgress, ParseChannelError,
};
pub use match_spec::MatchSpec;
pub use match_spec_constraints::MatchSpecConstraints;
pub use platform::{ParsePlatformError, Platform};
pub use repo_data::{ChannelInfo, NoArchType, PackageRecord, RepoData};
pub use solver::{PackageIndex, SolverIndex};
pub use version::{ParseVersionError, ParseVersionErrorKind, Version};
pub use version_spec::VersionSpec;
