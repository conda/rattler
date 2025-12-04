//! `rattler_solve` is a crate that provides functionality to solve Conda
//! environments. It currently exposes the functionality through the
//! [`SolverImpl::solve`] function.

#![deny(missing_docs)]

#[cfg(feature = "libsolv_c")]
pub mod libsolv_c;
#[cfg(feature = "resolvo")]
pub mod resolvo;

use std::collections::HashSet;
use std::fmt;

use jiff::Timestamp;
use rattler_conda_types::{
    GenericVirtualPackage, MatchSpec, PackageName, RepoDataRecord, SolverResult,
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

/// Configuration for filtering packages based on their minimum age.
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
/// use rattler_solve::MinimumAgeConfig;
///
/// // Only allow packages that have been published for at least 1 hour
/// let config = MinimumAgeConfig::new(Duration::from_secs(60 * 60))
///     // But allow "my-internal-package" to bypass this check
///     .with_exempt_package("my-internal-package".parse().unwrap());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinimumAgeConfig {
    /// The minimum age a package must have before it can be considered for
    /// installation. Packages published more recently than this duration ago
    /// will be excluded from the solve.
    ///
    /// For example, if set to 7 days, only packages that were published at
    /// least 7 days ago will be considered.
    pub min_age: std::time::Duration,

    /// The reference time to use when calculating the cutoff date.
    /// Packages published after `now - min_age` will be excluded.
    ///
    /// Defaults to the current time when [`MinimumAgeConfig::new`] is called.
    pub now: Timestamp,

    /// Packages that are exempt from the minimum release age requirement.
    ///
    /// This is useful for packages that you trust or that need to be updated
    /// frequently, even if they were recently published.
    pub exempt_packages: HashSet<PackageName>,

    /// Whether to include packages that don't have a timestamp.
    ///
    /// By default, packages without a timestamp are excluded when a minimum age
    /// filter is active. Set this to `true` to include them anyway.
    pub include_unknown_timestamp: bool,
}

impl Default for MinimumAgeConfig {
    fn default() -> Self {
        Self {
            min_age: std::time::Duration::default(),
            now: Timestamp::now(),
            exempt_packages: HashSet::new(),
            include_unknown_timestamp: false,
        }
    }
}

impl MinimumAgeConfig {
    /// Creates a new `MinimumAgeConfig` with the specified minimum age.
    /// The reference time (`now`) is set to the current time.
    pub fn new(min_age: std::time::Duration) -> Self {
        Self {
            min_age,
            now: Timestamp::now(),
            exempt_packages: HashSet::new(),
            include_unknown_timestamp: false,
        }
    }

    /// Sets the reference time to use when calculating the cutoff date.
    pub fn with_now(mut self, now: Timestamp) -> Self {
        self.now = now;
        self
    }

    /// Adds a package to the set of exempt packages.
    pub fn with_exempt_package(mut self, package: PackageName) -> Self {
        self.exempt_packages.insert(package);
        self
    }

    /// Sets the set of exempt packages.
    pub fn with_exempt_packages(mut self, packages: impl IntoIterator<Item = PackageName>) -> Self {
        self.exempt_packages = packages.into_iter().collect();
        self
    }

    /// Sets whether packages without a timestamp should be included.
    ///
    /// By default, packages without a timestamp are excluded. Call this with
    /// `true` to include them.
    pub fn with_include_unknown_timestamp(mut self, include: bool) -> Self {
        self.include_unknown_timestamp = include;
        self
    }

    /// Returns `true` if the given package is exempt from the minimum release
    /// age check.
    pub fn is_exempt(&self, package: &PackageName) -> bool {
        self.exempt_packages.contains(package)
    }

    /// Computes the cutoff time. Packages published after this time will be
    /// excluded (unless exempt).
    pub fn cutoff(&self) -> Timestamp {
        let span = jiff::Span::try_from(self.min_age)
            .expect("min_release_age duration is too large");
        self.now.checked_sub(span).expect("cutoff time overflow")
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

    /// Exclude any package that has a timestamp newer than the specified
    /// timestamp.
    pub exclude_newer: Option<Timestamp>,

    /// Only consider packages that have been published for at least the
    /// specified duration.
    ///
    /// This helps reduce the risk of installing compromised packages, as
    /// malicious releases are typically discovered and removed from channels
    /// within a short time window. By requiring a minimum age, you give the
    /// community time to identify and report malicious packages.
    ///
    /// Some packages can be exempted from this check using the
    /// [`MinimumAgeConfig::exempt_packages`] field.
    pub min_age: Option<MinimumAgeConfig>,

    /// The solve strategy.
    pub strategy: SolveStrategy,
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
            min_age: None,
            strategy: SolveStrategy::default(),
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
    /// number is used and downprioritization still works.
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
