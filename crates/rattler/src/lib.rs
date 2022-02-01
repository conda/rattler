mod channel;
mod match_spec;
mod platform;
mod repo_data;
pub(crate) mod utils;
mod version;
mod version_spec;

pub use channel::{Channel, ChannelConfig, ParseChannelError};
pub use platform::{ParsePlatformError, Platform};
pub use repo_data::{ChannelInfo, NoArchType, PackageRecord, RepoData};
pub use version::{ParseVersionError, ParseVersionErrorKind, VersionOrder};
pub use version_spec::VersionSpec;
