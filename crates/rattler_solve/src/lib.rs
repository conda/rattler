//! `rattler_solve` is a crate that provides functionality to solve Conda
//! environments. It currently exposes the functionality through the
//! [`SolverImpl::solve`] function.

#![deny(missing_docs)]

#[cfg(feature = "libsolv_c")]
pub mod libsolv_c;
#[cfg(feature = "resolvo")]
pub mod resolvo;

use std::collections::{HashMap, HashSet};
use std::fmt;

use chrono::{DateTime, Utc};
use rattler_conda_types::{
    utils::TimestampMs, GenericVirtualPackage, MatchSpec, PackageName, RepoDataRecord, SolverResult,
};

/// Represents a solver implementation, capable of solving [`SolverTask`]s
pub trait SolverImpl {
    /// The repo data associated to a channel and platform combination
    type RepoData<'a>: SolverRepoData<'a>;

    /// Resolve the dependencies and return the [`RepoDataRecord`]s that should
    /// be present in the environment.
    fn solve<
        'a,
        R: IntoRepoData<'a, Self::RepoData<'a>>,
        TAvailablePackagesIterator: IntoIterator<Item = R>,
    >(
        &mut self,
        task: SolverTask<TAvailablePackagesIterator>,
    ) -> Result<SolverResult, SolveError>;
}

/// Represents an error when solving the dependencies for a given environment
#[derive(thiserror::Error, Debug)]
pub enum SolveError {
    /// There is no set of dependencies that satisfies the requirements
    Unsolvable(Vec<String>),

    /// The solver backend returned operations that we dont know how to install.
    /// Each string is a somewhat user-friendly representation of which
    /// operation was not recognized and can be used for error reporting
    UnsupportedOperations(Vec<String>),

    /// Error when converting matchspec
    #[error(transparent)]
    ParseMatchSpecError(#[from] rattler_conda_types::ParseMatchSpecError),

    /// Encountered duplicate records in the available packages.
    DuplicateRecords(String),

    /// To support Resolvo cancellation
    Cancelled,
}

impl fmt::Display for SolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SolveError::Unsolvable(operations) => {
                write!(
                    f,
                    "Cannot solve the request because of: {}",
                    operations.join(", ")
                )
            }
            SolveError::UnsupportedOperations(operations) => {
                write!(f, "Unsupported operations: {}", operations.join(", "))
            }
            SolveError::ParseMatchSpecError(e) => {
                write!(f, "Error parsing match spec: {e}")
            }
            SolveError::Cancelled => {
                write!(f, "Solve operation has been cancelled")
            }
            SolveError::DuplicateRecords(filename) => {
                write!(f, "encountered duplicate records for {filename}")
            }
        }
    }
}

/// Configuration for filtering packages newer than a cutoff.
///
/// This feature helps reduce the risk of installing compromised packages by
/// delaying the installation of newly published versions. In most cases,
/// malicious releases are discovered and removed from channels within a short
/// time window (often within an hour). By requiring packages to have been
/// published for a minimum duration, you give the community time to identify
/// and report malicious packages before they can be installed.
///
/// This is similar to pnpm's `minimumReleaseAge` feature.
///
/// # Example
///
/// ```
/// use std::time::Duration;
/// use rattler_solve::ExcludeNewer;
///
/// // Only allow packages that have been published for at least 1 hour
/// let config = ExcludeNewer::from_duration(Duration::from_secs(60 * 60))
///     // But allow "my-internal-package" to bypass this check
///     .with_exempt_package("my-internal-package".parse().unwrap())
///     // And allow a trusted internal channel to skip the delay entirely
///     .with_channel_duration("my-internal-channel", Duration::ZERO);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExcludeNewer {
    /// Exclude packages uploaded after a fixed cutoff date.
    DateTime {
        /// The cutoff date. Packages uploaded after this date are excluded.
        cutoff: DateTime<Utc>,

        /// Channel-specific cutoff dates that override [`Self::DateTime::cutoff`]
        /// for records from matching channels.
        ///
        /// The key is matched against [`RepoDataRecord::channel`] exactly.
        channel_cutoffs: HashMap<String, DateTime<Utc>>,

        /// Packages that are exempt from the cutoff requirement.
        exempt_packages: HashSet<PackageName>,

        /// Whether to include packages that don't have a timestamp.
        ///
        /// By default, absolute cutoff dates keep packages with unknown
        /// timestamps to preserve the previous `exclude_newer` behavior.
        include_unknown_timestamp: bool,
    },

    /// Exclude packages uploaded more recently than a duration ago.
    Duration {
        /// The minimum age a package must have before it can be considered for
        /// installation. Packages published more recently than this duration
        /// ago will be excluded from the solve.
        duration: std::time::Duration,

        /// Channel-specific durations that override [`Self::Duration::duration`]
        /// for records from matching channels.
        ///
        /// The key is matched against [`RepoDataRecord::channel`] exactly.
        channel_durations: HashMap<String, std::time::Duration>,

        /// The reference time to use when calculating the cutoff date.
        /// Packages published after `now - duration` will be excluded.
        ///
        /// Defaults to the current time when [`ExcludeNewer::from_duration`] is
        /// called.
        now: DateTime<Utc>,

        /// Packages that are exempt from the cutoff requirement.
        exempt_packages: HashSet<PackageName>,

        /// Whether to include packages that don't have a timestamp.
        ///
        /// By default, duration-based cutoffs exclude packages without a
        /// timestamp to preserve the previous `min_age` behavior.
        include_unknown_timestamp: bool,
    },
}

impl ExcludeNewer {
    /// Creates a new configuration from an absolute cutoff date.
    pub fn from_datetime(cutoff: DateTime<Utc>) -> Self {
        Self::DateTime {
            cutoff,
            channel_cutoffs: HashMap::new(),
            exempt_packages: HashSet::new(),
            include_unknown_timestamp: false,
        }
    }

    /// Creates a new configuration from a relative duration.
    pub fn from_duration(duration: std::time::Duration) -> Self {
        Self::Duration {
            duration,
            channel_durations: HashMap::new(),
            now: Utc::now(),
            exempt_packages: HashSet::new(),
            include_unknown_timestamp: false,
        }
    }

    /// Sets the reference time to use when calculating duration-based cutoffs.
    pub fn with_now(mut self, now: DateTime<Utc>) -> Self {
        if let Self::Duration {
            now: current_now, ..
        } = &mut self
        {
            *current_now = now;
        }
        self
    }

    /// Adds a package to the set of exempt packages.
    pub fn with_exempt_package(mut self, package: PackageName) -> Self {
        match &mut self {
            Self::DateTime {
                exempt_packages, ..
            }
            | Self::Duration {
                exempt_packages, ..
            } => {
                exempt_packages.insert(package);
            }
        }
        self
    }

    /// Sets the set of exempt packages.
    pub fn with_exempt_packages(mut self, packages: impl IntoIterator<Item = PackageName>) -> Self {
        let packages = packages.into_iter().collect();
        match &mut self {
            Self::DateTime {
                exempt_packages, ..
            }
            | Self::Duration {
                exempt_packages, ..
            } => *exempt_packages = packages,
        }
        self
    }

    /// Sets the duration override for a specific channel.
    pub fn with_channel_duration(
        mut self,
        channel: impl Into<String>,
        duration: std::time::Duration,
    ) -> Self {
        if let Self::Duration {
            channel_durations, ..
        } = &mut self
        {
            channel_durations.insert(channel.into(), duration);
        }
        self
    }

    /// Sets the absolute cutoff override for a specific channel.
    pub fn with_channel_cutoff(
        mut self,
        channel: impl Into<String>,
        cutoff: DateTime<Utc>,
    ) -> Self {
        if let Self::DateTime {
            channel_cutoffs, ..
        } = &mut self
        {
            channel_cutoffs.insert(channel.into(), cutoff);
        }
        self
    }

    /// Sets whether packages without a timestamp should be included.
    ///
    /// By default, packages without a timestamp are excluded. Call this with
    /// `true` to include them.
    pub fn with_include_unknown_timestamp(mut self, include: bool) -> Self {
        match &mut self {
            Self::DateTime {
                include_unknown_timestamp,
                ..
            }
            | Self::Duration {
                include_unknown_timestamp,
                ..
            } => *include_unknown_timestamp = include,
        }
        self
    }

    /// Returns `true` if the given package is exempt from the minimum release
    /// age check.
    pub fn is_exempt(&self, package: &PackageName) -> bool {
        match self {
            Self::DateTime {
                exempt_packages, ..
            }
            | Self::Duration {
                exempt_packages, ..
            } => exempt_packages.contains(package),
        }
    }

    /// Returns whether packages without a timestamp are included.
    pub fn include_unknown_timestamp(&self) -> bool {
        match self {
            Self::DateTime {
                include_unknown_timestamp,
                ..
            }
            | Self::Duration {
                include_unknown_timestamp,
                ..
            } => *include_unknown_timestamp,
        }
    }

    /// Returns the effective duration for a record from the given channel.
    pub fn duration_for_channel(&self, channel: Option<&str>) -> Option<std::time::Duration> {
        match self {
            Self::DateTime { .. } => None,
            Self::Duration {
                duration,
                channel_durations,
                ..
            } => Some(
                channel
                    .and_then(|channel| channel_durations.get(channel).copied())
                    .unwrap_or(*duration),
            ),
        }
    }

    /// Computes the cutoff time for records from the given channel.
    pub fn cutoff_for_channel(&self, channel: Option<&str>) -> DateTime<Utc> {
        match self {
            Self::DateTime {
                cutoff,
                channel_cutoffs,
                ..
            } => channel
                .and_then(|channel| channel_cutoffs.get(channel).copied())
                .unwrap_or(*cutoff),
            Self::Duration { now, .. } => {
                let duration = chrono::Duration::from_std(
                    self.duration_for_channel(channel)
                        .expect("duration-based config must return a duration"),
                )
                .expect("exclude_newer duration is too large");
                *now - duration
            }
        }
    }

    /// Returns the reason a package should be excluded, if any.
    pub fn exclusion_reason(
        &self,
        package: &PackageName,
        channel: Option<&str>,
        timestamp: Option<&TimestampMs>,
    ) -> Option<String> {
        if self.is_exempt(package) {
            return None;
        }

        match timestamp {
            Some(timestamp) => {
                let cutoff = self.cutoff_for_channel(channel);
                if *timestamp <= cutoff {
                    return None;
                }

                Some(match self {
                    Self::DateTime { .. } => {
                        format!("the package is uploaded after the cutoff date of {cutoff}")
                    }
                    Self::Duration { .. } => {
                        let duration = humantime::format_duration(
                            self.duration_for_channel(channel)
                                .expect("duration-based config must return a duration"),
                        );
                        format!("the package was published less than {duration} ago")
                    }
                })
            }
            None if !self.include_unknown_timestamp() => {
                Some("the package has no timestamp".to_string())
            }
            None => None,
        }
    }
}

impl From<DateTime<Utc>> for ExcludeNewer {
    fn from(value: DateTime<Utc>) -> Self {
        Self::from_datetime(value)
    }
}

impl From<std::time::Duration> for ExcludeNewer {
    fn from(value: std::time::Duration) -> Self {
        Self::from_duration(value)
    }
}

/// Represents the channel priority option to use during solves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum ChannelPriority {
    /// The channel that the package is first found in will be used as the only
    /// channel for that package.
    #[default]
    Strict,

    // Conda also has "Flexible" as an option, where packages present in multiple channels
    // are only taken from lower-priority channels when this prevents unsatisfiable environment
    // errors, but this would need implementation in the solvers.
    // Flexible,
    /// Packages can be retrieved from any channel as package version takes
    /// precedence.
    Disabled,
}
#[derive(Debug, Clone, PartialEq, Eq)]
/// Represents a dependency resolution task, to be solved by one of the backends
pub struct SolverTask<TAvailablePackagesIterator> {
    /// An iterator over all available packages
    pub available_packages: TAvailablePackagesIterator,

    /// Records of packages that are previously selected.
    ///
    /// If the solver encounters multiple variants of a single package
    /// (identified by its name), it will sort the records and select the
    /// best possible version. However, if there exists a locked version it
    /// will prefer that variant instead. This is useful to reduce the number of
    /// packages that are updated when installing new packages.
    ///
    /// Usually you add the currently installed packages or packages from a
    /// lock-file here.
    pub locked_packages: Vec<RepoDataRecord>,

    /// Records of packages that are previously selected and CANNOT be changed.
    ///
    /// If the solver encounters multiple variants of a single package
    /// (identified by its name), it will sort the records and select the
    /// best possible version. However, if there is a variant available in
    /// the `pinned_packages` field it will always select that version no matter
    /// what even if that means other packages have to be downgraded.
    pub pinned_packages: Vec<RepoDataRecord>,

    /// Virtual packages considered active
    pub virtual_packages: Vec<GenericVirtualPackage>,

    /// The specs we want to solve
    pub specs: Vec<MatchSpec>,

    /// Additional constraints that should be satisfied by the solver.
    /// Packages included in the `constraints` are not necessarily
    /// installed, but they must be satisfied by the solution.
    pub constraints: Vec<MatchSpec>,

    /// The timeout after which the solver should stop
    pub timeout: Option<std::time::Duration>,

    /// The channel priority to solve with, either [`ChannelPriority::Strict`]
    /// or [`ChannelPriority::Disabled`]
    pub channel_priority: ChannelPriority,

    /// Exclude packages newer than the configured cutoff.
    ///
    /// This can be either:
    ///
    /// - a fixed cutoff date, equivalent to the historical `exclude_newer`
    ///   behavior; or
    /// - a relative duration, equivalent to the historical `min_age`
    ///   behavior.
    pub exclude_newer: Option<ExcludeNewer>,

    /// The solve strategy.
    pub strategy: SolveStrategy,

    /// Dependency overrides that replace dependencies of matching packages.
    pub dependency_overrides: Vec<(MatchSpec, MatchSpec)>,
}

impl<'r, I: IntoIterator<Item = &'r RepoDataRecord>> FromIterator<I>
    for SolverTask<Vec<RepoDataIter<I>>>
{
    fn from_iter<T: IntoIterator<Item = I>>(iter: T) -> Self {
        Self {
            available_packages: iter.into_iter().map(|iter| RepoDataIter(iter)).collect(),
            locked_packages: Vec::new(),
            pinned_packages: Vec::new(),
            virtual_packages: Vec::new(),
            specs: Vec::new(),
            constraints: Vec::new(),
            timeout: None,
            channel_priority: ChannelPriority::default(),
            exclude_newer: None,
            strategy: SolveStrategy::default(),
            dependency_overrides: Vec::new(),
        }
    }
}

/// Represents the strategy to use when solving dependencies
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum SolveStrategy {
    /// Resolve the highest version of each package.
    #[default]
    Highest,

    /// Resolve the lowest compatible version for each package.
    ///
    /// All candidates with the same version are still ordered the same as
    /// with `Default`. This ensures that the candidate with the highest build
    /// number is used and down-prioritization still works.
    LowestVersion,

    /// Resolve the lowest compatible version for direct dependencies but the
    /// highest for transitive dependencies. This is similar to `LowestVersion`
    /// but only for direct dependencies.
    LowestVersionDirect,
}

/// A representation of a collection of [`RepoDataRecord`] usable by a
/// [`SolverImpl`] implementation.
///
/// Some solvers might be able to cache the collection between different runs of
/// the solver which could potentially eliminate some overhead. This trait
/// enables creating a representation of the repodata that is most suitable for
/// a specific backend.
///
/// Some solvers may add additional functionality to their specific
/// implementation that enables caching the repodata to disk in an efficient way
/// (see [`crate::libsolv_c::RepoData`] for an example).
pub trait SolverRepoData<'a>: FromIterator<&'a RepoDataRecord> {}

/// Defines the ability to convert a type into [`SolverRepoData`].
pub trait IntoRepoData<'a, S: SolverRepoData<'a>> {
    /// Converts this instance into an instance of [`SolverRepoData`] which is
    /// consumable by a specific [`SolverImpl`] implementation.
    fn into(self) -> S;
}

impl<'a, S: SolverRepoData<'a>> IntoRepoData<'a, S> for &'a Vec<RepoDataRecord> {
    fn into(self) -> S {
        self.iter().collect()
    }
}

impl<'a, S: SolverRepoData<'a>> IntoRepoData<'a, S> for &'a [RepoDataRecord] {
    fn into(self) -> S {
        self.iter().collect()
    }
}

impl<'a, S: SolverRepoData<'a>> IntoRepoData<'a, S> for S {
    fn into(self) -> S {
        self
    }
}

/// A helper struct that implements `IntoRepoData` for anything that can
/// iterate over `RepoDataRecord`s.
pub struct RepoDataIter<T>(pub T);

impl<'a, T: IntoIterator<Item = &'a RepoDataRecord>, S: SolverRepoData<'a>> IntoRepoData<'a, S>
    for RepoDataIter<T>
{
    fn into(self) -> S {
        self.0.into_iter().collect()
    }
}
