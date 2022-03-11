mod channel;
mod match_spec;
mod match_spec_constraints;
mod platform;
mod range;
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

use range::Range;
