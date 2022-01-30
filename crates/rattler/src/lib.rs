mod channel;
mod platform;
mod repo_data;
mod version;
mod match_spec;
mod version_spec;
pub(crate) mod utils;

pub use channel::{Channel, ChannelConfig, ParseChannelError};
pub use platform::{Platform, ParsePlatformError};
pub use repo_data::{RepoData, PackageRecord, ChannelInfo, NoArchType};
pub use version::{VersionOrder, ParseVersionErrorKind, ParseVersionError};
pub use version_spec::VersionSpec;
