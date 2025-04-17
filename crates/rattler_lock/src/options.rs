// Reexport these fields.
pub use rattler_solve::{ChannelPriority, SolveStrategy};

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
}
