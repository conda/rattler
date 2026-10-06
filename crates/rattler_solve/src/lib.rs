//! `rattler_solve` is a crate that provides functionality to solve Conda
//! environments. It currently exposes the functionality through the
//! [`SolverImpl::solve`] function.

#![deny(missing_docs)]

#[cfg(feature = "libsolv_c")]
pub mod libsolv_c;
#[cfg(any(feature = "resolvo", feature = "libsolv_c"))]
mod priority_tier;
#[cfg(feature = "resolvo")]
pub mod resolvo;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use jiff::Timestamp;
use rattler_conda_types::{
    GenericVirtualPackage, MatchSpec, Matches, PackageName, RepoDataRecord, SolverResult,
};
use url::Url;

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
        task: SolverTask<'a, TAvailablePackagesIterator>,
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

/// A token that can be used to signal cancellation of an in-flight solve.
///
/// The token is cheap to clone and can be shared across threads. Calling
/// [`CancellationToken::cancel`] on any clone will signal all associated
/// solves to stop as soon as possible, in which case the solve returns
/// [`SolveError::Cancelled`].
///
/// # Backend support
///
/// Cancellation is currently only observed by the `resolvo` backend. Other
/// backends (such as `libsolv_c`) silently ignore the token and will run to
/// completion.
///
/// # Example
///
/// ```
/// use rattler_solve::CancellationToken;
///
/// let token = CancellationToken::new();
/// let token_clone = token.clone();
///
/// // From another thread / task, request cancellation:
/// std::thread::spawn(move || {
///     token_clone.cancel();
/// });
///
/// assert!(!token.is_cancelled() || token.is_cancelled());
/// ```
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl PartialEq for CancellationToken {
    /// Two tokens are considered equal when they share the same underlying
    /// cancellation state (i.e. one is a clone of the other).
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cancelled, &other.cancelled)
    }
}

impl Eq for CancellationToken {}

impl CancellationToken {
    /// Creates a new, un-cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Signals cancellation. All clones of this token will observe the
    /// cancelled state.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Returns `true` if cancellation has been requested on this token (or any
    /// of its clones).
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

/// Global timestamp selection and missing metadata policy for [`ExcludeNewer`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TimestampPolicy {
    /// Prefer index time, then build time; include records missing both.
    AllowMissing,
    /// Prefer index time, then build time; reject records missing both.
    #[default]
    RequireTimestamp,
    /// Use only index time; reject records without an index timestamp.
    RequireIndexedTimestamp,
}

/// Why a record is excluded by [`ExcludeNewer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TimestampExclusionReason {
    /// The strict policy requires publication metadata.
    #[error("the package has no indexed timestamp")]
    MissingIndexedTimestamp,
    /// Neither publication nor build time is available.
    #[error("the package has no timestamp")]
    MissingTimestamp,
    /// The selected timestamp is strictly later than the effective cutoff.
    #[error("the package is uploaded after the cutoff date of {}", cutoff.to_zoned(jiff::tz::TimeZone::system()).strftime("%Y-%m-%d %H:%M:%S"))]
    NewerThanCutoff {
        /// The effective package/channel cutoff.
        cutoff: Timestamp,
    },
}

/// Error returned by [`ExcludeNewer::with_exemption`] for a spec that cannot
/// be used as an exemption.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidExemptionError {
    /// The spec does not name exactly one package.
    #[error("exclude-newer exemption '{0}' must name exactly one package")]
    NotExactName(String),
    /// The spec has extras or a condition, which do not select records.
    #[error("exclude-newer exemption '{0}' must not have extras or a condition")]
    UnsupportedField(String),
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
/// By default, [`TimestampPolicy::RequireTimestamp`] uses the index timestamp,
/// falls back to build time, and excludes records missing both. Use
/// [`Self::with_timestamp_policy`] to change this for all packages and channels.
/// Records exactly at their effective cutoff remain eligible.
///
/// Records matching an exemption added with [`Self::with_exemption`] are
/// never excluded, regardless of their timestamp. This allows a single vetted
/// release (for example an urgent security fix) without lowering the cutoff
/// for every future release of that package.
///
/// # Example
///
/// ```
/// use std::time::Duration;
/// use rattler_solve::ExcludeNewer;
///
/// // Only allow packages that have been published for at least 1 hour
/// let config = ExcludeNewer::from_duration(Duration::from_secs(60 * 60))
///     // And allow a trusted internal channel to skip the delay entirely
///     .with_channel_duration("my-internal-channel", Duration::ZERO);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludeNewer {
    /// The default cutoff date. Packages uploaded after this date are excluded.
    cutoff: Timestamp,

    /// Channel-specific cutoff dates that override [`Self::cutoff`] for
    /// records from matching channels.
    ///
    /// The key is matched against [`RepoDataRecord::channel`] exactly.
    channel_cutoffs: HashMap<String, Timestamp>,

    /// Package-specific cutoff dates that override both [`Self::cutoff`] and
    /// [`Self::channel_cutoffs`] for matching package names.
    ///
    /// Deprecated in favor of [`Self::exemptions`].
    package_cutoffs: HashMap<PackageName, Timestamp>,

    /// Records matching any of these specs are never excluded.
    exemptions: HashSet<MatchSpec>,

    /// Timestamp policy shared by all packages and channels.
    timestamp_policy: TimestampPolicy,
}

impl ExcludeNewer {
    fn cutoff_from_duration(duration: std::time::Duration, now: Timestamp) -> Timestamp {
        let span = jiff::Span::try_from(duration).expect("exclude_newer duration is too large");
        now.checked_sub(span)
            .expect("exclude_newer duration subtraction overflowed")
    }

    /// Creates a new configuration from an absolute cutoff date.
    pub fn from_datetime(cutoff: Timestamp) -> Self {
        Self {
            cutoff,
            channel_cutoffs: HashMap::new(),
            package_cutoffs: HashMap::new(),
            exemptions: HashSet::new(),
            timestamp_policy: TimestampPolicy::default(),
        }
    }

    /// Creates a new configuration from a relative duration.
    pub fn from_duration(duration: std::time::Duration) -> Self {
        Self::from_duration_with_now(duration, Timestamp::now())
    }

    /// Creates a new configuration from a relative duration and explicit
    /// reference time.
    pub fn from_duration_with_now(duration: std::time::Duration, now: Timestamp) -> Self {
        Self {
            cutoff: Self::cutoff_from_duration(duration, now),
            channel_cutoffs: HashMap::new(),
            package_cutoffs: HashMap::new(),
            exemptions: HashSet::new(),
            timestamp_policy: TimestampPolicy::default(),
        }
    }

    /// Sets the absolute cutoff override for a specific package.
    #[deprecated(
        note = "use `with_exemption` to allow specific vetted releases instead of lowering the cutoff for every release of a package"
    )]
    pub fn with_package_cutoff(mut self, package: PackageName, cutoff: Timestamp) -> Self {
        self.package_cutoffs.insert(package, cutoff);
        self
    }

    /// Sets the duration override for a specific package.
    #[deprecated(
        note = "use `with_exemption` to allow specific vetted releases instead of lowering the cutoff for every release of a package"
    )]
    pub fn with_package_duration(
        mut self,
        package: PackageName,
        duration: std::time::Duration,
    ) -> Self {
        self.package_cutoffs.insert(
            package,
            Self::cutoff_from_duration(duration, Timestamp::now()),
        );
        self
    }

    /// Sets the duration override for a specific package using an explicit
    /// reference time.
    #[deprecated(
        note = "use `with_exemption` to allow specific vetted releases instead of lowering the cutoff for every release of a package"
    )]
    pub fn with_package_duration_with_now(
        mut self,
        package: PackageName,
        duration: std::time::Duration,
        now: Timestamp,
    ) -> Self {
        self.package_cutoffs
            .insert(package, Self::cutoff_from_duration(duration, now));
        self
    }

    /// Sets the duration override for a specific channel.
    pub fn with_channel_duration(
        mut self,
        channel: impl Into<String>,
        duration: std::time::Duration,
    ) -> Self {
        self.channel_cutoffs.insert(
            channel.into(),
            Self::cutoff_from_duration(duration, Timestamp::now()),
        );
        self
    }

    /// Sets the duration override for a specific channel using an explicit
    /// reference time.
    pub fn with_channel_duration_with_now(
        mut self,
        channel: impl Into<String>,
        duration: std::time::Duration,
        now: Timestamp,
    ) -> Self {
        self.channel_cutoffs
            .insert(channel.into(), Self::cutoff_from_duration(duration, now));
        self
    }

    /// Sets the absolute cutoff override for a specific channel.
    pub fn with_channel_cutoff(mut self, channel: impl Into<String>, cutoff: Timestamp) -> Self {
        self.channel_cutoffs.insert(channel.into(), cutoff);
        self
    }

    /// Exempts records matching `spec` from the cutoff and the timestamp
    /// policy.
    ///
    /// The spec must name exactly one package; a spec without a name or with
    /// a glob or regex name would exempt more than intended. Extras and
    /// conditions are rejected because matching ignores them. If the spec has
    /// a channel, only records from exactly that channel are exempt.
    pub fn with_exemption(mut self, spec: MatchSpec) -> Result<Self, InvalidExemptionError> {
        if spec.name.as_exact().is_none() {
            return Err(InvalidExemptionError::NotExactName(spec.to_string()));
        }
        if spec.extras.is_some() || spec.condition.is_some() {
            return Err(InvalidExemptionError::UnsupportedField(spec.to_string()));
        }
        self.exemptions.insert(spec);
        Ok(self)
    }

    /// Returns whether a record matches one of the exemptions.
    pub fn is_exempt(&self, record: &RepoDataRecord) -> bool {
        self.exemptions.iter().any(|spec| {
            // `MatchSpec::matches` ignores the channel, so check it here.
            let channel_matches = spec.channel.as_ref().is_none_or(|channel| {
                record.channel.as_deref() == Some(channel.canonical_name().as_str())
            });
            channel_matches && spec.matches(record)
        })
    }

    /// Sets the global timestamp policy. Cutoff overrides do not change it.
    pub fn with_timestamp_policy(mut self, policy: TimestampPolicy) -> Self {
        self.timestamp_policy = policy;
        self
    }

    /// Returns the global timestamp policy.
    pub fn timestamp_policy(&self) -> TimestampPolicy {
        self.timestamp_policy
    }

    /// Computes the cutoff time for the given package and channel.
    pub fn cutoff_for_package(&self, package: &PackageName, channel: Option<&str>) -> Timestamp {
        self.package_cutoffs
            .get(package)
            .copied()
            .or_else(|| channel.and_then(|channel| self.channel_cutoffs.get(channel).copied()))
            .unwrap_or(self.cutoff)
    }

    /// Returns why a record is excluded, preserving timestamp provenance.
    pub fn exclusion_reason(&self, record: &RepoDataRecord) -> Option<TimestampExclusionReason> {
        if self.is_exempt(record) {
            return None;
        }
        let package = &record.package_record;
        let timestamp = match self.timestamp_policy {
            TimestampPolicy::RequireIndexedTimestamp => match package.indexed_timestamp {
                Some(timestamp) => Some(timestamp),
                None => return Some(TimestampExclusionReason::MissingIndexedTimestamp),
            },
            _ => package.indexed_timestamp.or(package.timestamp),
        };
        let cutoff = self.cutoff_for_package(&package.name, record.channel.as_deref());
        match timestamp {
            Some(timestamp) if timestamp > cutoff => {
                Some(TimestampExclusionReason::NewerThanCutoff { cutoff })
            }
            None if self.timestamp_policy == TimestampPolicy::RequireTimestamp => {
                Some(TimestampExclusionReason::MissingTimestamp)
            }
            _ => None,
        }
    }

    /// Returns whether a record should be excluded by the timestamp policy and cutoff.
    pub fn is_excluded(&self, record: &RepoDataRecord) -> bool {
        self.exclusion_reason(record).is_some()
    }
}

impl From<Timestamp> for ExcludeNewer {
    fn from(value: Timestamp) -> Self {
        Self::from_datetime(value)
    }
}

impl From<std::time::Duration> for ExcludeNewer {
    fn from(value: std::time::Duration) -> Self {
        Self::from_duration(value)
    }
}

/// Represents the channel priority option to use during solves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum ChannelPriority {
    /// The channel that the package is first found in will be used as the only
    /// channel for that package.
    #[default]
    Strict,

    /// Packages can be retrieved from any channel as package version takes
    /// precedence.
    Disabled,

    /// For a given package, candidates from higher-priority channel's are exhausted
    /// before falling back to the next channel, regardless of the version.
    Flexible,
}
#[derive(Debug, Clone, PartialEq, Eq)]
/// Represents a dependency resolution task, to be solved by one of the backends
pub struct SolverTask<'a, TAvailablePackagesIterator> {
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
    ///
    /// Records are passed by reference so the caller keeps ownership of the
    /// underlying storage (whether that is `Vec<RepoDataRecord>`,
    /// `Vec<Arc<RepoDataRecord>>` from the gateway, or anything else).
    pub locked_packages: Vec<&'a RepoDataRecord>,

    /// Records of packages that are previously selected and CANNOT be changed.
    ///
    /// If the solver encounters multiple variants of a single package
    /// (identified by its name), it will sort the records and select the
    /// best possible version. However, if there is a variant available in
    /// the `pinned_packages` field it will always select that version no matter
    /// what even if that means other packages have to be downgraded.
    ///
    /// See [`Self::locked_packages`] for the borrowing rationale.
    pub pinned_packages: Vec<&'a RepoDataRecord>,

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

    /// Candidates the solver may not select, keyed by the record's URL and
    /// mapped to the reason they were ruled out.
    ///
    /// The reason is reported back as part of the error when the solve fails
    /// because of it, so it should read as an explanation to a user, e.g.
    /// "not available locally".
    ///
    /// This is meant for restrictions the caller derives from outside the
    /// repodata, such as what is present in a local package cache. Records
    /// that are simply irrelevant should be left out of `available_packages`
    /// instead; excluding a record keeps it visible to error reporting, which
    /// is the whole point of using this rather than filtering up front.
    ///
    /// The reason is shared rather than owned per entry, because callers
    /// typically rule out thousands of records for the same handful of reasons.
    ///
    /// A record that is both excluded and pinned through `pinned_packages`
    /// makes the solve unsatisfiable when the package is needed, and a record
    /// that is excluded and favored through `locked_packages` stays excluded:
    /// an exclusion is never overruled by a preference.
    ///
    /// Only the `resolvo` backend supports this. Other backends reject a task
    /// that sets it rather than silently solving without the restriction.
    pub excluded_candidates: HashMap<Url, Arc<str>>,

    /// An optional token that can be used to cancel an in-flight solve.
    ///
    /// When the token's [`CancellationToken::cancel`] method is invoked from
    /// another thread, the solver stops as soon as possible and returns
    /// [`SolveError::Cancelled`].
    ///
    /// Only the `resolvo` backend observes this token. Other backends
    /// ignore it.
    pub cancellation_token: Option<CancellationToken>,
}

impl<'r, T> SolverTask<'r, T> {
    /// Creates a task that solves against `available_packages` with default
    /// settings for everything else.
    fn with_available_packages(available_packages: T) -> Self {
        Self {
            available_packages,
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
            excluded_candidates: HashMap::new(),
            cancellation_token: None,
        }
    }
}

impl<'r, I: IntoIterator<Item = &'r RepoDataRecord>> FromIterator<I>
    for SolverTask<'r, Vec<RepoDataIter<I>>>
{
    fn from_iter<T: IntoIterator<Item = I>>(iter: T) -> Self {
        Self::with_available_packages(iter.into_iter().map(|iter| RepoDataIter(iter)).collect())
    }
}

impl<'r, I: IntoIterator<Item = &'r RepoDataRecord>> FromIterator<ChannelRepoData<'r, I>>
    for SolverTask<'r, Vec<ChannelRepoData<'r, I>>>
{
    fn from_iter<T: IntoIterator<Item = ChannelRepoData<'r, I>>>(iter: T) -> Self {
        Self::with_available_packages(iter.into_iter().collect())
    }
}

/// Represents the strategy to use when solving dependencies
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash)]
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
pub trait SolverRepoData<'a>: FromIterator<&'a RepoDataRecord> {
    /// Places these records in the multichannel called `name`.
    ///
    /// See [`ChannelRepoData`] for how a multichannel affects the solve.
    fn set_multi_channel(&mut self, name: &'a str);
}

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

/// The records of a single channel subdirectory, together with the
/// multichannel the channel was requested through, if any.
///
/// All channels of one multichannel form a single channel priority tier, like
/// conda treats them. With [`ChannelPriority::Strict`] a package is not
/// excluded from a later member just because an earlier member also provides
/// that package, while channels outside the multichannel are still excluded.
/// With [`ChannelPriority::Flexible`] the solver does not prefer an earlier
/// member over a later one. The order of the members only breaks ties between
/// otherwise identical candidates.
///
/// For backends that support channel-specific match specs, a spec whose
/// channel name equals the name of the multichannel (e.g. `defaults::python`)
/// accepts records from every member.
pub struct ChannelRepoData<'a, T> {
    /// The records of the channel subdirectory.
    pub records: T,

    /// The name of the multichannel this channel belongs to, or `None` if the
    /// channel was requested on its own.
    pub multi_channel: Option<&'a str>,
}

impl<'a, T: IntoIterator<Item = &'a RepoDataRecord>, S: SolverRepoData<'a>> IntoRepoData<'a, S>
    for ChannelRepoData<'a, T>
{
    fn into(self) -> S {
        let mut repo_data: S = self.records.into_iter().collect();
        if let Some(name) = self.multi_channel {
            repo_data.set_multi_channel(name);
        }
        repo_data
    }
}

#[cfg(test)]
mod tests {
    use rattler_conda_types::ParseMatchSpecOptions;

    use super::*;

    #[test]
    fn exemption_requires_exact_name() {
        let config = ExcludeNewer::from_datetime(Timestamp::UNIX_EPOCH);
        let options = ParseMatchSpecOptions::lenient().with_exact_names_only(false);
        for spec in ["pkg-*", "*", "^pkg-.*$"] {
            let spec = MatchSpec::from_str(spec, options).unwrap();
            assert!(
                matches!(
                    config.clone().with_exemption(spec.clone()),
                    Err(InvalidExemptionError::NotExactName(_))
                ),
                "expected '{spec}' to be rejected"
            );
        }
        let options = ParseMatchSpecOptions::strict()
            .with_extras(true)
            .with_conditionals(true);
        for spec in ["pkg[extras=[foo]]", r#"pkg[when="other"]"#] {
            let spec = MatchSpec::from_str(spec, options).unwrap();
            assert!(
                matches!(
                    config.clone().with_exemption(spec.clone()),
                    Err(InvalidExemptionError::UnsupportedField(_))
                ),
                "expected '{spec}' to be rejected"
            );
        }

        let spec = MatchSpec::from_str("pkg ==1.0", ParseMatchSpecOptions::strict()).unwrap();
        assert!(config.with_exemption(spec).is_ok());
    }
}
