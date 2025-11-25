//! Provides an solver implementation based on the [`resolvo`] crate.

use std::{
    cell::RefCell,
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fmt::{Display, Formatter},
    marker::PhantomData,
};

use chrono::{DateTime, Utc};
use conda_sorting::SolvableSorter;
use itertools::Itertools;
use rattler_conda_types::MatchSpecCondition;
use rattler_conda_types::{
    package::ArchiveType, utils::TimestampMs, GenericVirtualPackage, MatchSpec, Matches,
    NamelessMatchSpec, PackageName, PackageNameMatcher, ParseMatchSpecError, ParseMatchSpecOptions,
    RepoDataRecord, SolverResult,
};
use resolvo::{
    utils::{Pool, VersionSet},
    Candidates, Condition, ConditionId, ConditionalRequirement, Dependencies, DependencyProvider,
    HintDependenciesAvailable, Interner, KnownDependencies, NameId, Problem, SolvableId,
    Solver as LibSolvRsSolver, SolverCache, StringId, UnsolvableOrCancelled, VersionSetId,
    VersionSetUnionId,
};

use crate::{
    resolvo::conda_sorting::CompareStrategy, ChannelPriority, IntoRepoData, SolveError,
    SolveStrategy, SolverRepoData, SolverTask,
};

mod conda_sorting;

type MatchSpecParseCache = HashMap<String, (Vec<VersionSetId>, Option<ConditionId>)>;

/// Represents the information required to load available packages into libsolv
/// for a single channel and platform combination
#[derive(Clone)]
pub struct RepoData<'a> {
    /// The actual records after parsing `repodata.json`
    pub records: Vec<&'a RepoDataRecord>,
}

impl<'a> FromIterator<&'a RepoDataRecord> for RepoData<'a> {
    fn from_iter<T: IntoIterator<Item = &'a RepoDataRecord>>(iter: T) -> Self {
        Self {
            records: Vec::from_iter(iter),
        }
    }
}

impl<'a> SolverRepoData<'a> for RepoData<'a> {}

/// Wrapper around `MatchSpec` so that we can use it in the `resolvo` pool
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum SolverMatchSpec<'a> {
    /// Represents a requirement on another package.
    MatchSpec(NamelessMatchSpec),

    /// The name already uniquely identifies a single package. So we don't need
    /// any special "spec". here.
    Extra,

    /// A helper variant to make sure we can add a lifetime to this enum.
    _Phantom(PhantomData<&'a ()>),
}

impl From<NamelessMatchSpec> for SolverMatchSpec<'_> {
    fn from(value: NamelessMatchSpec) -> Self {
        SolverMatchSpec::MatchSpec(value)
    }
}

impl Display for SolverMatchSpec<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SolverMatchSpec::MatchSpec(spec) => {
                write!(f, "{spec}")
            }
            SolverMatchSpec::Extra => Ok(()),
            SolverMatchSpec::_Phantom(_) => unreachable!(),
        }
    }
}

impl<'a> VersionSet for SolverMatchSpec<'a> {
    type V = SolverPackageRecord<'a>;
}

/// Wrapper around [`RepoDataRecord`] so that we can use it in resolvo pool.
/// Also represents a virtual package or an extra of a package.
#[derive(Eq, PartialEq)]
pub enum SolverPackageRecord<'a> {
    /// Represents a record from the repodata
    Record(&'a RepoDataRecord),

    /// Represents a virtual package.
    VirtualPackage(&'a GenericVirtualPackage),

    /// Represents a named extra for a particular package name. e.g.
    /// `numpy[blas]`
    Extra {
        /// The name of the package
        package: PackageName,

        /// The extra to activate in the package
        extra: String,
    },
}

impl PartialOrd<Self> for SolverPackageRecord<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SolverPackageRecord<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name()
            .cmp(other.name())
            .then_with(|| self.extra().cmp(&other.extra()))
            .then_with(|| self.version().cmp(&other.version()))
            .then_with(|| self.build_number().cmp(&other.build_number()))
            .then_with(|| self.timestamp().cmp(&other.timestamp()))
    }
}

impl SolverPackageRecord<'_> {
    fn name(&self) -> &PackageName {
        match self {
            SolverPackageRecord::Record(rec) => &rec.package_record.name,
            SolverPackageRecord::Extra { package, .. } => package,
            SolverPackageRecord::VirtualPackage(rec) => &rec.name,
        }
    }

    fn extra(&self) -> Option<&String> {
        match self {
            SolverPackageRecord::Extra { extra, .. } => Some(extra),
            SolverPackageRecord::Record(_) | SolverPackageRecord::VirtualPackage(_) => None,
        }
    }

    fn version(&self) -> Option<&rattler_conda_types::Version> {
        match self {
            SolverPackageRecord::Record(rec) => Some(rec.package_record.version.version()),
            SolverPackageRecord::VirtualPackage(rec) => Some(&rec.version),
            SolverPackageRecord::Extra { .. } => None,
        }
    }

    fn track_features(&self) -> &[String] {
        const EMPTY: [String; 0] = [];
        match self {
            SolverPackageRecord::Record(rec) => &rec.package_record.track_features,
            SolverPackageRecord::Extra { .. } | SolverPackageRecord::VirtualPackage(..) => &EMPTY,
        }
    }

    fn build_number(&self) -> u64 {
        match self {
            SolverPackageRecord::Record(rec) => rec.package_record.build_number,
            SolverPackageRecord::Extra { .. } | SolverPackageRecord::VirtualPackage(..) => 0,
        }
    }

    fn timestamp(&self) -> Option<&chrono::DateTime<chrono::Utc>> {
        match self {
            SolverPackageRecord::Record(rec) => rec
                .package_record
                .timestamp
                .as_ref()
                .map(TimestampMs::datetime),
            SolverPackageRecord::Extra { .. } | SolverPackageRecord::VirtualPackage(..) => None,
        }
    }
}

impl Display for SolverPackageRecord<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SolverPackageRecord::Record(rec) => {
                write!(f, "{}", &rec.package_record)
            }
            SolverPackageRecord::Extra { package, extra } => {
                write!(f, "{}[{}]", package.as_normalized(), extra)
            }
            SolverPackageRecord::VirtualPackage(rec) => {
                write!(f, "{rec}")
            }
        }
    }
}

/// Represents the type of name that is being used in the pool.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum NameType {
    /// A simple package
    Base(String),

    /// An extra of a package (e.g. `numpy[blas]`)
    Extra {
        /// The package name
        package: String,

        /// The extra to activate.
        extra: String,
    },
}

impl Display for NameType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            NameType::Base(name) => write!(f, "{name}"),
            NameType::Extra { package, extra } => write!(f, "{package}[{extra}]"),
        }
    }
}

impl Ord for NameType {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            // Compare names first, then extras
            (NameType::Base(name1), NameType::Base(name2)) => name1.cmp(name2),
            (
                NameType::Extra {
                    package: a,
                    extra: extra_a,
                },
                NameType::Extra {
                    package: b,
                    extra: extra_b,
                },
            ) => a.cmp(b).then_with(|| extra_a.cmp(extra_b)),
            (NameType::Base(_), NameType::Extra { .. }) => std::cmp::Ordering::Greater,
            (NameType::Extra { .. }, NameType::Base(_)) => std::cmp::Ordering::Less,
        }
    }
}

impl PartialOrd for NameType {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl From<&PackageName> for NameType {
    fn from(value: &PackageName) -> Self {
        NameType::Base(value.as_normalized().to_owned())
    }
}

/// An implement of [`resolvo::DependencyProvider`] that implements the
/// ecosystem behavior for conda. This allows resolvo to solve for conda
/// packages.
#[derive(Default)]
pub struct CondaDependencyProvider<'a> {
    /// The pool that deduplicates data used by the provider.
    pub pool: Pool<SolverMatchSpec<'a>, NameType>,
    name_to_condition: RefCell<HashMap<NameId, ConditionId>>,

    /// Holds all the cached candidates for each package name.
    records: HashMap<NameId, Candidates>,

    matchspec_to_highest_version:
        RefCell<HashMap<VersionSetId, Option<(rattler_conda_types::Version, bool)>>>,

    parse_match_spec_cache: RefCell<MatchSpecParseCache>,

    stop_time: Option<std::time::SystemTime>,

    strategy: SolveStrategy,

    direct_dependencies: HashSet<NameId>,
}

impl<'a> CondaDependencyProvider<'a> {
    /// Constructs a new provider.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repodata: impl IntoIterator<Item = RepoData<'a>>,
        favored_records: &'a [RepoDataRecord],
        locked_records: &'a [RepoDataRecord],
        virtual_packages: &'a [GenericVirtualPackage],
        match_specs: &[MatchSpec],
        stop_time: Option<std::time::SystemTime>,
        channel_priority: ChannelPriority,
        exclude_newer: Option<DateTime<Utc>>,
        strategy: SolveStrategy,
    ) -> Result<Self, SolveError> {
        let pool = Pool::default();
        let mut records: HashMap<NameId, Candidates> = HashMap::default();

        // Add virtual packages to the records
        for virtual_package in virtual_packages {
            let name = pool.intern_package_name(&virtual_package.name);
            let solvable =
                pool.intern_solvable(name, SolverPackageRecord::VirtualPackage(virtual_package));
            records.entry(name).or_default().candidates.push(solvable);
        }

        // Compute the direct dependencies
        let direct_dependencies = match_specs
            .iter()
            .filter_map(|spec| spec.name.as_ref())
            .filter_map(|name| Option::<PackageName>::from(name.clone()))
            .map(|name| pool.intern_package_name(&name))
            .collect();

        // TODO: Normalize these channel names to urls so we can compare them correctly.
        let channel_specific_specs = match_specs
            .iter()
            .filter(|spec| spec.channel.is_some())
            .collect::<Vec<_>>();

        // Hashmap that maps the package name to the channel it was first found in.
        let mut package_name_found_in_channel = HashMap::<String, &Option<String>>::new();

        // Add additional records
        for repo_data in repodata {
            // Iterate over all records and dedup records that refer to the same package
            // data but with different archive types. This can happen if you
            // have two variants of the same package but with different
            // extensions. We prefer `.conda` packages over `.tar.bz`.
            //
            // Its important to insert the records in the same order as how they were
            // presented to this function to ensure that each solve is
            // deterministic. Iterating over HashMaps is not deterministic at
            // runtime so instead we store the values in a Vec as we iterate over the
            // records. This guarantees that the order of records remains the same over
            // runs.
            let mut ordered_repodata = Vec::with_capacity(repo_data.records.len());
            let mut package_to_type: HashMap<&str, (ArchiveType, usize, bool)> =
                HashMap::with_capacity(repo_data.records.len());

            for record in repo_data.records {
                // Determine if this record will be excluded.
                let excluded = matches!((&exclude_newer, &record.package_record.timestamp),
                    (Some(exclude_newer), Some(record_timestamp))
                        if record_timestamp > exclude_newer);

                let (file_name, archive_type) = ArchiveType::split_str(&record.file_name)
                    .unwrap_or((&record.file_name, ArchiveType::TarBz2));
                match package_to_type.get_mut(file_name) {
                    None => {
                        let idx = ordered_repodata.len();
                        ordered_repodata.push(record);
                        package_to_type.insert(file_name, (archive_type, idx, excluded));
                    }
                    Some((prev_archive_type, idx, previous_excluded)) => {
                        if *previous_excluded && !excluded {
                            // The previous package would have been excluded by the solver. If the
                            // current record won't be excluded we should always use that.
                            *prev_archive_type = archive_type;
                            ordered_repodata[*idx] = record;
                            *previous_excluded = false;
                        } else if excluded && !*previous_excluded {
                            // The previous package would not have been excluded
                            // by the solver but
                            // this one will, so we'll keep the previous one
                            // regardless of the type.
                        } else {
                            match archive_type.cmp(prev_archive_type) {
                                Ordering::Greater => {
                                    // A previous package has a worse package "type", we'll use the
                                    // current record instead.
                                    *prev_archive_type = archive_type;
                                    ordered_repodata[*idx] = record;
                                    *previous_excluded = excluded;
                                }
                                Ordering::Less => {
                                    // A previous package that we already stored
                                    // is actually a package of a better
                                    // "type" so we'll just use that instead
                                    // (.conda > .tar.bz)
                                }
                                Ordering::Equal => {
                                    return Err(SolveError::DuplicateRecords(
                                        record.file_name.clone(),
                                    ));
                                }
                            }
                        }
                    }
                }
            }

            for record in ordered_repodata {
                let package_name = pool.intern_package_name(&record.package_record.name);
                let solvable_id =
                    pool.intern_solvable(package_name, SolverPackageRecord::Record(record));

                // Update records with all entries in a single mutable borrow
                let candidates = records.entry(package_name).or_default();
                candidates.candidates.push(solvable_id);

                // Filter out any records that are newer than a specific date.
                match (&exclude_newer, &record.package_record.timestamp) {
                    (Some(exclude_newer), Some(record_timestamp))
                        if record_timestamp > exclude_newer =>
                    {
                        let reason = pool.intern_string(format!(
                            "the package is uploaded after the cutoff date of {exclude_newer}"
                        ));
                        candidates.excluded.push((solvable_id, reason));
                    }
                    _ => {}
                }

                // Add to excluded when package is not in the specified channel.
                if !channel_specific_specs.is_empty() {
                    if let Some(spec) = channel_specific_specs.iter().find(|&&spec| {
                        spec.name
                            .as_ref()
                            .and_then(|name| Option::<PackageName>::from(name.clone()))
                            .expect("expecting an exact package name")
                            .as_normalized()
                            == record.package_record.name.as_normalized()
                    }) {
                        // Check if the spec has a channel, and compare it to the repodata
                        // channel
                        if let Some(spec_channel) = &spec.channel {
                            if record.channel.as_ref() != Some(&spec_channel.canonical_name()) {
                                tracing::debug!("Ignoring {} {} because it was not requested from that channel.", &record.package_record.name.as_normalized(), match &record.channel {
                                        Some(channel) => format!("from {}", &channel),
                                        None => "without a channel".to_string(),
                                    });
                                // Add record to the excluded with reason of being in the non
                                // requested channel.
                                let message = format!(
                                    "candidate not in requested channel: '{}'",
                                    spec_channel
                                        .name
                                        .clone()
                                        .unwrap_or(spec_channel.base_url.to_string())
                                );
                                candidates
                                    .excluded
                                    .push((solvable_id, pool.intern_string(message)));
                                continue;
                            }
                        }
                    }
                }

                // Enforce channel priority
                if let (Some(first_channel), ChannelPriority::Strict) = (
                    package_name_found_in_channel.get(record.package_record.name.as_normalized()),
                    channel_priority,
                ) {
                    // Add the record to the excluded list when it is from a different channel.
                    if first_channel != &&record.channel {
                        if let Some(channel) = &record.channel {
                            tracing::debug!(
                                "Ignoring '{}' from '{}' because of strict channel priority.",
                                &record.package_record.name.as_normalized(),
                                channel
                            );
                            candidates.excluded.push((
                                solvable_id,
                                pool.intern_string(format!(
                                    "due to strict channel priority not using this option from: '{channel}'",
                                )),
                            ));
                        } else {
                            tracing::debug!(
                                    "Ignoring '{}' without a channel because of strict channel priority.",
                                    &record.package_record.name.as_normalized(),
                                );
                            candidates.excluded.push((
                                solvable_id,
                                pool.intern_string("due to strict channel priority not using from an unknown channel".to_string()),
                            ));
                        }
                    }
                } else {
                    package_name_found_in_channel.insert(
                        record.package_record.name.as_normalized().to_string(),
                        &record.channel,
                    );
                }
            }
        }

        // Add favored packages to the records
        for favored_record in favored_records {
            let name = pool.intern_package_name(&favored_record.package_record.name);
            let solvable = pool.intern_solvable(name, SolverPackageRecord::Record(favored_record));
            let candidates = records.entry(name).or_default();
            candidates.candidates.push(solvable);
            candidates.favored = Some(solvable);
        }

        for locked_record in locked_records {
            let name = pool.intern_package_name(&locked_record.package_record.name);
            let solvable = pool.intern_solvable(name, SolverPackageRecord::Record(locked_record));
            let candidates = records.entry(name).or_default();
            candidates.candidates.push(solvable);
            candidates.locked = Some(solvable);
        }

        // The dependencies for all candidates are always available.
        for candidates in records.values_mut() {
            candidates.hint_dependencies_available = HintDependenciesAvailable::All;
        }

        Ok(Self {
            pool,
            name_to_condition: RefCell::default(),
            records,
            matchspec_to_highest_version: RefCell::default(),
            parse_match_spec_cache: RefCell::default(),
            stop_time,
            strategy,
            direct_dependencies,
        })
    }

    /// Returns all package names
    pub fn package_names(&self) -> impl Iterator<Item = NameId> + use<'_, 'a> {
        self.records.keys().copied()
    }

    fn extra_condition(&self, package: &PackageName, extra: &str) -> ConditionId {
        let name_id = self.pool.intern_package_name(NameType::Extra {
            package: package.as_normalized().to_owned(),
            extra: extra.to_owned(),
        });
        let mut name_to_condition = self.name_to_condition.borrow_mut();
        *name_to_condition.entry(name_id).or_insert_with(|| {
            let version_set = extra_version_set(&self.pool, package.clone(), extra.to_owned());
            self.pool
                .intern_condition(Condition::Requirement(version_set))
        })
    }
}

/// The reason why the solver was cancelled
pub enum CancelReason {
    /// The solver was cancelled because the timeout was reached
    Timeout,
}

impl Interner for CondaDependencyProvider<'_> {
    fn display_solvable(&self, solvable: SolvableId) -> impl Display + '_ {
        &self.pool.resolve_solvable(solvable).record
    }

    fn resolve_condition(&self, condition: ConditionId) -> Condition {
        self.pool.resolve_condition(condition).clone()
    }

    fn version_sets_in_union(
        &self,
        version_set_union: VersionSetUnionId,
    ) -> impl Iterator<Item = VersionSetId> {
        self.pool.resolve_version_set_union(version_set_union)
    }

    fn display_merged_solvables(&self, solvables: &[SolvableId]) -> impl Display + '_ {
        if solvables.is_empty() {
            return String::new();
        }

        let versions = solvables
            .iter()
            .filter_map(|&id| self.pool.resolve_solvable(id).record.version())
            .sorted()
            .format(" | ");

        let name = self.display_solvable_name(solvables[0]);
        let result = format!("{name} {versions}");
        result.trim_end().to_string()
    }

    fn display_name(&self, name: NameId) -> impl Display + '_ {
        self.pool.resolve_package_name(name)
    }

    fn display_version_set(&self, version_set: VersionSetId) -> impl Display + '_ {
        self.pool.resolve_version_set(version_set)
    }

    fn display_string(&self, string_id: StringId) -> impl Display + '_ {
        self.pool.resolve_string(string_id)
    }

    fn version_set_name(&self, version_set: VersionSetId) -> NameId {
        self.pool.resolve_version_set_package_name(version_set)
    }

    fn solvable_name(&self, solvable: SolvableId) -> NameId {
        self.pool.resolve_solvable(solvable).name
    }
}

impl DependencyProvider for CondaDependencyProvider<'_> {
    async fn sort_candidates(&self, solver: &SolverCache<Self>, solvables: &mut [SolvableId]) {
        if solvables.is_empty() {
            // Short circuit if there are no solvables to sort
            return;
        }

        let mut highest_version_spec = self.matchspec_to_highest_version.borrow_mut();

        let (strategy, dependency_strategy) = match self.strategy {
            SolveStrategy::Highest => (CompareStrategy::Default, CompareStrategy::Default),
            SolveStrategy::LowestVersion => (
                CompareStrategy::LowestVersion,
                CompareStrategy::LowestVersion,
            ),
            SolveStrategy::LowestVersionDirect => {
                if self
                    .direct_dependencies
                    .contains(&self.pool.resolve_solvable(solvables[0]).name)
                {
                    (CompareStrategy::LowestVersion, CompareStrategy::Default)
                } else {
                    (CompareStrategy::Default, CompareStrategy::Default)
                }
            }
        };

        // Custom sorter that sorts by name, version, and build
        // and then by the maximalization of dependency versions
        // more information can be found at the struct location
        SolvableSorter::new(solver, strategy, dependency_strategy)
            .sort(solvables, &mut highest_version_spec);
    }

    async fn get_candidates(&self, name: NameId) -> Option<Candidates> {
        match self.pool.resolve_package_name(name) {
            NameType::Base(_) => self.records.get(&name).cloned(),
            NameType::Extra { package, extra } => {
                // For extras, we need to create a new candidates object
                // that contains only the extra solvable.
                let extra_solvable = add_extra(
                    &self.pool,
                    PackageName::new_unchecked(package),
                    extra.clone(),
                );
                Some(Candidates {
                    candidates: vec![extra_solvable],
                    favored: None,
                    locked: None,
                    excluded: Vec::new(),
                    hint_dependencies_available: HintDependenciesAvailable::All,
                })
            }
        }
    }

    async fn get_dependencies(&self, solvable: SolvableId) -> Dependencies {
        let mut dependencies = KnownDependencies::default();

        let record = match &self.pool.resolve_solvable(solvable).record {
            SolverPackageRecord::Record(rec) => rec,
            SolverPackageRecord::Extra { .. } | SolverPackageRecord::VirtualPackage(_) => {
                return Dependencies::Known(dependencies)
            }
        };

        let mut parse_match_spec_cache = self.parse_match_spec_cache.borrow_mut();

        // Add regular dependencies
        for depends in record.package_record.depends.iter() {
            let specs = match parse_match_spec(&self.pool, depends, &mut parse_match_spec_cache) {
                Ok(version_set_id) => version_set_id,
                Err(e) => {
                    let reason = self
                        .pool
                        .intern_string(format!("the dependency '{depends}' failed to parse: {e}",));

                    return Dependencies::Unknown(reason);
                }
            };

            let (version_set_ids, condition_id) = specs;
            dependencies
                .requirements
                .extend(
                    version_set_ids
                        .into_iter()
                        .map(|id| ConditionalRequirement {
                            requirement: id.into(),
                            condition: condition_id,
                        }),
                );
        }

        // Add constraints from the record
        for constrains in record.package_record.constrains.iter() {
            let (version_set_ids, condition_id) =
                match parse_match_spec(&self.pool, constrains, &mut parse_match_spec_cache) {
                    Ok(version_set_id) => version_set_id,
                    Err(e) => {
                        let reason = self.pool.intern_string(format!(
                            "the constrains '{constrains}' failed to parse: {e}",
                        ));

                        return Dependencies::Unknown(reason);
                    }
                };
            if condition_id.is_some() {
                tracing::warn!("The package '{name}' has a constraint with a condition '{constrains}'. This is not supported by the solver and will be ignored.", name = record.package_record.name.as_normalized(), constrains = constrains);
            }
            dependencies.constrains.extend(version_set_ids);
        }

        // Add extras
        for (extra, matchspec) in record
            .package_record
            .experimental_extra_depends
            .iter()
            .flat_map(|(extra, deps)| deps.iter().map(move |dep| (extra, dep)))
        {
            let (version_set_ids, spec_condition) =
                match parse_match_spec(&self.pool, matchspec, &mut parse_match_spec_cache) {
                    Ok(version_set_id) => version_set_id,
                    Err(e) => {
                        let reason = self.pool.intern_string(format!(
                            "the constrains '{matchspec}' failed to parse: {e}",
                        ));

                        return Dependencies::Unknown(reason);
                    }
                };

            // Add them as conditional requirements (e.g. `numpy; if extra`).
            let extra_condition = self.extra_condition(&record.package_record.name, extra);
            for version_set_id in version_set_ids {
                dependencies.requirements.push(ConditionalRequirement {
                    requirement: version_set_id.into(),
                    condition: if let Some(condition) = spec_condition {
                        let condition = resolvo::Condition::Binary(
                            resolvo::LogicalOperator::And,
                            extra_condition,
                            condition,
                        );
                        Some(self.pool.intern_condition(condition))
                    } else {
                        Some(extra_condition)
                    },
                });
            }
        }

        Dependencies::Known(dependencies)
    }

    async fn filter_candidates(
        &self,
        candidates: &[SolvableId],
        version_set: VersionSetId,
        inverse: bool,
    ) -> Vec<SolvableId> {
        let spec = self.pool.resolve_version_set(version_set);
        match spec {
            SolverMatchSpec::MatchSpec(spec) => {
                candidates
                    .iter()
                    .copied()
                    .filter(|c| {
                        let record = &self.pool.resolve_solvable(*c).record;
                        match record {
                            SolverPackageRecord::Record(rec) => {
                                // Base package matches if spec matches and no features are required
                                spec.matches(*rec) != inverse
                            }
                            SolverPackageRecord::VirtualPackage(GenericVirtualPackage {
                                version,
                                build_string,
                                ..
                            }) => {
                                if let Some(spec) = spec.version.as_ref() {
                                    if !spec.matches(version) {
                                        return inverse;
                                    }
                                }

                                if let Some(build_match) = spec.build.as_ref() {
                                    if !build_match.matches(build_string) {
                                        return inverse;
                                    }
                                }

                                !inverse
                            }
                            SolverPackageRecord::Extra { .. } => {
                                unreachable!("extras should never be compared to matchspecs")
                            }
                        }
                    })
                    .collect()
            }
            SolverMatchSpec::Extra => {
                // Extras are already filtered by name.
                if inverse {
                    Vec::new()
                } else {
                    candidates.to_vec()
                }
            }
            SolverMatchSpec::_Phantom(_) => unreachable!(),
        }
    }

    fn should_cancel_with_value(&self) -> Option<Box<dyn std::any::Any>> {
        if let Some(stop_time) = self.stop_time {
            if std::time::SystemTime::now() > stop_time {
                return Some(Box::new(CancelReason::Timeout));
            }
        }
        None
    }
}

/// A [`Solver`] implemented using the `resolvo` library
#[derive(Default)]
pub struct Solver;

impl super::SolverImpl for Solver {
    type RepoData<'a> = RepoData<'a>;

    #[allow(clippy::redundant_closure_for_method_calls)]
    fn solve<
        'a,
        R: IntoRepoData<'a, Self::RepoData<'a>>,
        TAvailablePackagesIterator: IntoIterator<Item = R>,
    >(
        &mut self,
        task: SolverTask<TAvailablePackagesIterator>,
    ) -> Result<SolverResult, SolveError> {
        let stop_time = task
            .timeout
            .map(|timeout| std::time::SystemTime::now() + timeout);

        // Construct a provider that can serve the data.
        let provider = CondaDependencyProvider::new(
            task.available_packages.into_iter().map(|r| r.into()),
            &task.locked_packages,
            &task.pinned_packages,
            &task.virtual_packages,
            task.specs.clone().as_ref(),
            stop_time,
            task.channel_priority,
            task.exclude_newer,
            task.strategy,
        )?;

        // Construct the requirements that the solver needs to satisfy.
        let virtual_package_requirements = task.virtual_packages.iter().map(|spec| {
            let name_id = provider.pool.intern_package_name(&spec.name);
            provider
                .pool
                .intern_version_set(name_id, NamelessMatchSpec::default().into())
        });

        let root_requirements = task.specs.into_iter().flat_map(|spec| {
            let condition_id = if let Some(condition) = spec.condition.as_ref() {
                let mut cache = provider.parse_match_spec_cache.borrow_mut();
                Some(parse_condition(condition, &provider.pool, &mut cache))
            } else {
                None
            };

            version_sets_for_match_spec(&provider.pool, spec)
                .into_iter()
                .map(move |version_set_id| ConditionalRequirement {
                    requirement: version_set_id.into(),
                    condition: condition_id,
                })
        });

        let all_requirements: Vec<_> = virtual_package_requirements
            .map(ConditionalRequirement::from)
            .chain(root_requirements)
            .collect();

        let root_constraints = task
            .constraints
            .iter()
            .map(|spec| {
                let (Some(PackageNameMatcher::Exact(name)), spec) = spec.clone().into_nameless()
                else {
                    unimplemented!("only exact package names are supported");
                };
                let name_id = provider.pool.intern_package_name(&name);
                provider.pool.intern_version_set(name_id, spec.into())
            })
            .collect();

        let problem = Problem::new()
            .requirements(all_requirements.clone())
            .constraints(root_constraints);

        // Construct a solver and solve the problems in the queue
        let mut solver = LibSolvRsSolver::new(provider);
        let solvables = solver.solve(problem).map_err(|unsolvable_or_cancelled| {
            match unsolvable_or_cancelled {
                UnsolvableOrCancelled::Unsolvable(problem) => {
                    SolveError::Unsolvable(vec![problem.display_user_friendly(&solver).to_string()])
                }
                // We are not doing this as of yet
                // put a generic message in here for now
                UnsolvableOrCancelled::Cancelled(_) => SolveError::Cancelled,
            }
        })?;

        // Get the resulting packages from the solver.
        let mut extras: HashMap<PackageName, Vec<String>> = HashMap::new();
        let mut records = Vec::new();

        for id in solvables {
            match &solver.provider().pool.resolve_solvable(id).record {
                SolverPackageRecord::Record(rec) => {
                    records.push((*rec).clone());
                }
                SolverPackageRecord::Extra { package, extra } => {
                    extras
                        .entry(package.clone())
                        .or_default()
                        .push(extra.clone());
                }
                SolverPackageRecord::VirtualPackage(_) => {}
            }
        }

        Ok(SolverResult { records, extras })
    }
}

fn parse_match_spec(
    pool: &Pool<SolverMatchSpec<'_>, NameType>,
    spec_str: &str,
    parse_match_spec_cache: &mut MatchSpecParseCache,
) -> Result<(Vec<VersionSetId>, Option<ConditionId>), ParseMatchSpecError> {
    if let Some(cached) = parse_match_spec_cache.get(spec_str) {
        return Ok(cached.clone());
    }

    // Parse the match spec and extract the name of the package it depends on.
    // Enable conditionals parsing to support dependencies with conditions like "numpy; if python >=3.9"
    let match_spec = MatchSpec::from_str(
        spec_str,
        ParseMatchSpecOptions::lenient().with_experimental_conditionals(true),
    )?;
    let condition_id = if let Some(condition) = match_spec.condition.as_ref() {
        let condition_id = parse_condition(condition, pool, parse_match_spec_cache);
        Some(condition_id)
    } else {
        None
    };

    // Get the version sets for the match spec.
    let version_set_ids = version_sets_for_match_spec(pool, match_spec);

    // Store in the match spec cache
    parse_match_spec_cache.insert(
        spec_str.to_string(),
        (version_set_ids.clone(), condition_id),
    );

    Ok((version_set_ids, condition_id))
}

fn version_sets_for_match_spec(
    pool: &Pool<SolverMatchSpec<'_>, NameType>,
    spec: MatchSpec,
) -> Vec<VersionSetId> {
    let (Some(PackageNameMatcher::Exact(name)), spec) = spec.into_nameless() else {
        unimplemented!("only exact package names are supported");
    };

    // Add a dependency on each extra.
    let mut version_set_ids = vec![];
    for extra in spec.extras.iter().flatten() {
        version_set_ids.push(extra_version_set(pool, name.clone(), extra.clone()));
    }

    // Create a version set for the match spec itself.
    let dependency_name = pool.intern_package_name(&name);
    let version_set_id = pool.intern_version_set(dependency_name, spec.into());
    version_set_ids.push(version_set_id);

    version_set_ids
}

/// Adds a particular "extra" to the set of solvables
pub fn add_extra(
    pool: &Pool<SolverMatchSpec<'_>, NameType>,
    package_name: PackageName,
    extra: String,
) -> SolvableId {
    let name = NameType::Extra {
        package: package_name.as_normalized().to_owned(),
        extra: extra.clone(),
    };
    let name_id = pool.intern_package_name(name);

    // Ensure that there is a single solvable for this extra.
    pool.intern_solvable(
        name_id,
        SolverPackageRecord::Extra {
            package: package_name,
            extra,
        },
    )
}

/// Returns a version set for a particular package name and extra.
pub fn extra_version_set(
    pool: &Pool<SolverMatchSpec<'_>, NameType>,
    package_name: PackageName,
    extra: String,
) -> VersionSetId {
    let name = NameType::Extra {
        package: package_name.as_normalized().to_owned(),
        extra,
    };
    let name_id = pool.intern_package_name(name);
    pool.intern_version_set(name_id, SolverMatchSpec::Extra)
}

/// Parses a condition from a `MatchSpecCondition` and returns the corresponding `ConditionId`.
fn parse_condition(
    condition: &MatchSpecCondition,
    pool: &Pool<SolverMatchSpec<'_>, NameType>,
    parse_match_spec_cache: &mut MatchSpecParseCache,
) -> ConditionId {
    match condition {
        MatchSpecCondition::MatchSpec(match_spec) => {
            // Parse the match spec and intern it
            let (spec, condition) =
                parse_match_spec(pool, &match_spec.to_string(), parse_match_spec_cache).unwrap();
            if let Some(_condition) = condition {
                panic!("conditions cannot be nested");
            }
            let conditions = spec.into_iter().map(resolvo::Condition::Requirement);
            // Intern the conditions
            let condition_ids = conditions
                .into_iter()
                .map(|c| pool.intern_condition(c))
                .collect_vec();
            // Create a union of the conditions
            if condition_ids.is_empty() {
                panic!("match spec condition must have at least one version set");
            } else if condition_ids.len() == 1 {
                return condition_ids[0];
            } else {
                // Otherwise, create a union of the conditions
                let mut result = condition_ids[0];
                for &condition_id in &condition_ids[1..] {
                    let union_condition = resolvo::Condition::Binary(
                        resolvo::LogicalOperator::And,
                        result,
                        condition_id,
                    );
                    result = pool.intern_condition(union_condition);
                }
                result
            }
        }
        MatchSpecCondition::And(left, right) => {
            let condition_id_lhs = parse_condition(left, pool, parse_match_spec_cache);
            let condition_id_rhs = parse_condition(right, pool, parse_match_spec_cache);
            // Intern the AND condition
            let condition = resolvo::Condition::Binary(
                resolvo::LogicalOperator::And,
                condition_id_lhs,
                condition_id_rhs,
            );
            pool.intern_condition(condition)
        }
        MatchSpecCondition::Or(left, right) => {
            let condition_id_lhs = parse_condition(left, pool, parse_match_spec_cache);
            let condition_id_rhs = parse_condition(right, pool, parse_match_spec_cache);
            // Intern the OR condition
            let condition = resolvo::Condition::Binary(
                resolvo::LogicalOperator::Or,
                condition_id_lhs,
                condition_id_rhs,
            );
            pool.intern_condition(condition)
        }
    }
}
