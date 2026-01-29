//! Defines [`SolveOptions`] and reexports from `rattler_solve` that are used.

// Reexport these fields.
pub use rattler_solve::{ChannelPriority, SolveStrategy};

/// The prerelease mode used to resolve `PyPI` dependencies.
///
/// This controls how the resolver handles pre-release versions of packages.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
#[serde(rename_all = "kebab-case")]
pub enum PypiPrereleaseMode {
    /// Disallow all pre-release versions.
    Disallow,
    /// Allow all pre-release versions.
    Allow,
    /// Allow pre-release versions if necessary to satisfy the requirements.
    IfNecessary,
    /// Allow pre-release versions for packages explicitly requested.
    Explicit,
    /// Allow pre-release versions if necessary or explicitly requested.
    /// This is the default mode.
    #[default]
    IfNecessaryOrExplicit,
}

/// Options that were used during the resolution of the packages stored in the
/// lock-file. These options strongly influence the outcome of the solve and are
/// therefore stored along with the locked packages.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SolveOptions {
    /// The strategy that was used to solve the dependencies.
    #[serde(default, skip_serializing_if = "crate::utils::serde::is_default")]
    pub strategy: SolveStrategy,

    /// The channel priority that was used to solve the dependencies.
    #[serde(default, skip_serializing_if = "crate::utils::serde::is_default")]
    pub channel_priority: ChannelPriority,

    /// Packages after this date have been excluded from the lock file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_newer: Option<chrono::DateTime<chrono::Utc>>,

    /// The prerelease mode that was used to resolve `PyPI` dependencies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pypi_prerelease_mode: Option<PypiPrereleaseMode>,
}
