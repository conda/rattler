//! Provides an solver implementation based on the [`resolvo`] crate.

use std::{
    cell::RefCell,
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fmt::{Display, Formatter},
    marker::PhantomData,
    ops::Deref,
};

use chrono::{DateTime, Utc};
use conda_sorting::SolvableSorter;
use itertools::Itertools;
use rattler_conda_types::{
    package::ArchiveType, version_spec::EqualityOperator, BuildNumberSpec, GenericVirtualPackage,
    MatchSpec, Matches, NamelessMatchSpec, OrdOperator, PackageName, PackageRecord,
    ParseMatchSpecError, ParseStrictness, RepoDataRecord, SolverResult, StringMatcher, VersionSpec,
};
use resolvo::{
    utils::{Pool, VersionSet},
    Candidates, Dependencies, DependencyProvider, Interner, KnownDependencies, NameId, Problem,
    Requirement, SolvableId, Solver as LibSolvRsSolver, SolverCache, StringId,
    UnsolvableOrCancelled, VersionSetId, VersionSetUnionId,
};

use crate::{
    resolvo::conda_sorting::CompareStrategy, ChannelPriority, IntoRepoData, SolveError,
    SolveStrategy, SolverRepoData, SolverTask,
};

mod conda_sorting;

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
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct SolverMatchSpec<'a> {
    inner: NamelessMatchSpec,
    feature: Option<String>,
    _marker: PhantomData<&'a PackageRecord>,
}

impl<'a> SolverMatchSpec<'a> {
    /// Returns a reference to this match spec with the given feature enabled
    pub fn with_feature(&self, feature: String) -> SolverMatchSpec<'a> {
        Self {
            inner: self.inner.clone(),
            feature: Some(feature),
            _marker: self._marker,
        }
    }

    /// Returns a mutable reference to this match spec after enabling the given feature
    pub fn set_feature(&mut self, feature: String) -> &SolverMatchSpec<'a> {
        self.feature = Some(feature);
        self
    }
}

impl From<NamelessMatchSpec> for SolverMatchSpec<'_> {
    fn from(value: NamelessMatchSpec) -> Self {
        Self {
            inner: value,
            feature: None,
            _marker: PhantomData,
        }
    }
}

impl Display for SolverMatchSpec<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl Deref for SolverMatchSpec<'_> {
    type Target = NamelessMatchSpec;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'a> VersionSet for SolverMatchSpec<'a> {
    type V = SolverPackageRecord<'a>;
}

/// Wrapper around [`PackageRecord`] so that we can use it in resolvo pool
#[derive(Eq, PartialEq)]
pub enum SolverPackageRecord<'a> {
    /// Represents a record from the repodata
    Record(&'a RepoDataRecord),

    /// Represents a record with a specific feature enabled
    RecordWithFeature(&'a RepoDataRecord, String),

    /// Represents a virtual package.
    VirtualPackage(&'a GenericVirtualPackage),
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
            .then_with(|| self.version().cmp(other.version()))
            .then_with(|| self.build_number().cmp(&other.build_number()))
            .then_with(|| self.timestamp().cmp(&other.timestamp()))
    }
}

impl SolverPackageRecord<'_> {
    fn name(&self) -> &PackageName {
        match self {
            SolverPackageRecord::Record(rec) | SolverPackageRecord::RecordWithFeature(rec, _) => {
                &rec.package_record.name
            }
            SolverPackageRecord::VirtualPackage(rec) => &rec.name,
        }
    }

    fn version(&self) -> &rattler_conda_types::Version {
        match self {
            SolverPackageRecord::Record(rec) | SolverPackageRecord::RecordWithFeature(rec, _) => {
                rec.package_record.version.version()
            }
            SolverPackageRecord::VirtualPackage(rec) => &rec.version,
        }
    }

    fn track_features(&self) -> &[String] {
        const EMPTY: [String; 0] = [];
        match self {
            SolverPackageRecord::Record(rec) | SolverPackageRecord::RecordWithFeature(rec, _) => {
                &rec.package_record.track_features
            }
            SolverPackageRecord::VirtualPackage(_rec) => &EMPTY,
        }
    }

    fn build_number(&self) -> u64 {
        match self {
            SolverPackageRecord::Record(rec) | SolverPackageRecord::RecordWithFeature(rec, _) => {
                rec.package_record.build_number
            }
            SolverPackageRecord::VirtualPackage(_rec) => 0,
        }
    }

    fn timestamp(&self) -> Option<&chrono::DateTime<chrono::Utc>> {
        match self {
            SolverPackageRecord::Record(rec) | SolverPackageRecord::RecordWithFeature(rec, _) => {
                rec.package_record.timestamp.as_ref()
            }
            SolverPackageRecord::VirtualPackage(_rec) => None,
        }
    }
}

impl Display for SolverPackageRecord<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SolverPackageRecord::Record(rec) => {
                write!(f, "{}", &rec.package_record)
            }
            SolverPackageRecord::RecordWithFeature(rec, feature) => {
                write!(f, "{}[{}]", &rec.package_record, feature)
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
    /// A package name without a feature.
    Base(String),

    /// A package name with a feature.
    BaseWithFeature(String, String),
}

impl Display for NameType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            NameType::Base(name) => write!(f, "{name}"),
            NameType::BaseWithFeature(name, feature) => write!(f, "{name}[{feature}]"),
        }
    }
}

impl Ord for NameType {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            // Compare names first
            (NameType::Base(name1), NameType::Base(name2))
            | (NameType::BaseWithFeature(name1, _), NameType::BaseWithFeature(name2, _)) => {
                name1.cmp(name2)
            }
            // WithoutFeature comes before WithFeature
            (NameType::Base(_), NameType::BaseWithFeature(_, _)) => std::cmp::Ordering::Greater,
            (NameType::BaseWithFeature(_, _), NameType::Base(_)) => std::cmp::Ordering::Less,
        }
    }
}

impl PartialOrd for NameType {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl From<&str> for NameType {
    fn from(value: &str) -> Self {
        NameType::Base(value.to_owned())
    }
}

/// An implement of [`resolvo::DependencyProvider`] that implements the
/// ecosystem behavior for conda. This allows resolvo to solve for conda
/// packages.
#[derive(Default)]
pub struct CondaDependencyProvider<'a> {
    /// The pool that deduplicates data used by the provider.
    pub pool: Pool<SolverMatchSpec<'a>, NameType>,

    records: HashMap<NameId, Candidates>,

    matchspec_to_highest_version:
        RefCell<HashMap<VersionSetId, Option<(rattler_conda_types::Version, bool)>>>,

    parse_match_spec_cache: RefCell<HashMap<String, Vec<VersionSetId>>>,

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
            let name = pool.intern_package_name(virtual_package.name.as_normalized());
            let solvable =
                pool.intern_solvable(name, SolverPackageRecord::VirtualPackage(virtual_package));
            records.entry(name).or_default().candidates.push(solvable);
        }

        // Compute the direct dependencies
        let direct_dependencies = match_specs
            .iter()
            .filter_map(|spec| spec.name.as_ref())
            .map(|name| pool.intern_package_name(name.as_normalized()))
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
                let package_name =
                    pool.intern_package_name(record.package_record.name.as_normalized());
                let solvable_id =
                    pool.intern_solvable(package_name, SolverPackageRecord::Record(record));

                // Collect all candidates first
                let mut all_entries = vec![(package_name, solvable_id)];

                // Add feature-enabled solvables
                for feature in record.package_record.extra_depends.keys() {
                    let package_name_with_feature =
                        pool.intern_package_name(NameType::BaseWithFeature(
                            record.package_record.name.as_normalized().to_owned(),
                            feature.clone(),
                        ));
                    let feature_solvable = pool.intern_solvable(
                        package_name_with_feature,
                        SolverPackageRecord::RecordWithFeature(record, feature.clone()),
                    );
                    let package_name_with_feature =
                        pool.intern_package_name(NameType::BaseWithFeature(
                            record.package_record.name.as_normalized().to_owned(),
                            feature.clone(),
                        ));

                    all_entries.push((package_name_with_feature, feature_solvable));
                }

                // Update records with all entries in a single mutable borrow
                for (pkg_name, solvable) in all_entries {
                    let candidates = records.entry(pkg_name).or_default();
                    candidates.candidates.push(solvable);

                    // Filter out any records that are newer than a specific date.
                    match (&exclude_newer, &record.package_record.timestamp) {
                        (Some(exclude_newer), Some(record_timestamp))
                            if record_timestamp > exclude_newer =>
                        {
                            let reason = pool.intern_string(format!(
                                "the package is uploaded after the cutoff date of {exclude_newer}"
                            ));
                            candidates.excluded.push((solvable, reason));
                        }
                        _ => {}
                    }

                    // Add to excluded when package is not in the specified channel.
                    if !channel_specific_specs.is_empty() {
                        if let Some(spec) = channel_specific_specs.iter().find(|&&spec| {
                            spec.name
                                .as_ref()
                                .expect("expecting a name")
                                .as_normalized()
                                == record.package_record.name.as_normalized()
                        }) {
                            // Check if the spec has a channel, and compare it to the repodata channel
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
                                        .push((solvable, pool.intern_string(message)));
                                    continue;
                                }
                            }
                        }
                    }

                    // Enforce channel priority
                    if let (Some(first_channel), ChannelPriority::Strict) = (
                        package_name_found_in_channel
                            .get(record.package_record.name.as_normalized()),
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
                                    solvable,
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
                                    solvable,
                                    pool.intern_string("due to strict channel priority not using from an unknown channel".to_string()),
                                ));
                            }
                            continue;
                        }
                    } else {
                        package_name_found_in_channel.insert(
                            record.package_record.name.as_normalized().to_string(),
                            &record.channel,
                        );
                    }
                }
            }
        }

        // Add favored packages to the records
        for favored_record in favored_records {
            let name = pool.intern_package_name(favored_record.package_record.name.as_normalized());
            let solvable = pool.intern_solvable(name, SolverPackageRecord::Record(favored_record));
            let candidates = records.entry(name).or_default();
            candidates.candidates.push(solvable);
            candidates.favored = Some(solvable);
        }

        for locked_record in locked_records {
            let name = pool.intern_package_name(locked_record.package_record.name.as_normalized());
            let solvable = pool.intern_solvable(name, SolverPackageRecord::Record(locked_record));
            let candidates = records.entry(name).or_default();
            candidates.candidates.push(solvable);
            candidates.locked = Some(solvable);
        }

        // The dependencies for all candidates are always available.
        for candidates in records.values_mut() {
            candidates.hint_dependencies_available = candidates.candidates.clone();
        }

        Ok(Self {
            pool,
            records,
            matchspec_to_highest_version: RefCell::default(),
            parse_match_spec_cache: RefCell::default(),
            stop_time,
            strategy,
            direct_dependencies,
        })
    }

    /// Returns all package names
    pub fn package_names(&self) -> impl Iterator<Item = NameId> + '_ {
        self.records.keys().copied()
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
            .map(|&id| self.pool.resolve_solvable(id).record.version())
            .sorted()
            .format(" | ");

        let name = self.display_solvable_name(solvables[0]);
        format!("{name} {versions}")
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
        self.records.get(&name).cloned()
    }

    async fn get_dependencies(&self, solvable: SolvableId) -> Dependencies {
        let mut dependencies = KnownDependencies::default();

        // Get the record and any feature that might be enabled
        let (record, feature) = match &self.pool.resolve_solvable(solvable).record {
            SolverPackageRecord::Record(rec) => (rec, None),
            SolverPackageRecord::RecordWithFeature(rec, feature) => (rec, Some(feature)),
            SolverPackageRecord::VirtualPackage(_) => return Dependencies::Known(dependencies),
        };

        let mut parse_match_spec_cache = self.parse_match_spec_cache.borrow_mut();

        // If this is a feature-enabled package, add its feature dependencies
        if let Some(feature_name) = feature {
            // Find the feature's dependencies
            if let Some(deps) = record.package_record.extra_depends.get(feature_name) {
                // Add each dependency for this feature
                for req in deps {
                    let version_set_id = match parse_match_spec(
                        &self.pool,
                        req,
                        &mut parse_match_spec_cache,
                    ) {
                        Ok(version_set_id) => version_set_id,
                        Err(e) => {
                            let reason = self.pool.intern_string(format!(
                                "the optional dependency '{req}' for feature '{feature_name}' failed to parse: {e}"
                            ));
                            return Dependencies::Unknown(reason);
                        }
                    };

                    dependencies
                        .requirements
                        .extend(version_set_id.into_iter().map(Requirement::from));
                }

                // Add a dependency back to the base package with exact version
                let base_spec = MatchSpec {
                    name: Some(record.package_record.name.clone()),
                    version: Some(VersionSpec::Exact(
                        EqualityOperator::Equals,
                        record.package_record.version.version().clone(),
                    )),
                    build: Some(StringMatcher::Exact(record.package_record.build.clone())),
                    build_number: Some(BuildNumberSpec::new(
                        OrdOperator::Eq,
                        record.package_record.build_number,
                    )),
                    subdir: Some(record.package_record.subdir.clone()),
                    md5: record.package_record.md5,
                    sha256: record.package_record.sha256,
                    extras: None,
                    ..Default::default()
                };

                let (name, nameless_spec) = base_spec.into_nameless();
                let name_id = self.pool.intern_package_name(
                    name.expect("cannot use matchspec without a name")
                        .as_normalized(),
                );
                let version_set_id = self.pool.intern_version_set(name_id, nameless_spec.into());
                dependencies.requirements.push(version_set_id.into());
            }
        } else {
            // Add regular dependencies
            for depends in record.package_record.depends.iter() {
                let version_set_id =
                    match parse_match_spec(&self.pool, depends, &mut parse_match_spec_cache) {
                        Ok(version_set_id) => version_set_id,
                        Err(e) => {
                            let reason = self.pool.intern_string(format!(
                                "the dependency '{depends}' failed to parse: {e}",
                            ));

                            return Dependencies::Unknown(reason);
                        }
                    };

                dependencies
                    .requirements
                    .extend(version_set_id.into_iter().map(Requirement::from));
            }

            for constrains in record.package_record.constrains.iter() {
                let version_set_id =
                    match parse_match_spec(&self.pool, constrains, &mut parse_match_spec_cache) {
                        Ok(version_set_id) => version_set_id,
                        Err(e) => {
                            let reason = self.pool.intern_string(format!(
                                "the constrains '{constrains}' failed to parse: {e}",
                            ));

                            return Dependencies::Unknown(reason);
                        }
                    };
                for version_set_id in version_set_id {
                    dependencies.constrains.push(version_set_id);
                }
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
                    SolverPackageRecord::RecordWithFeature(rec, feature) => {
                        // Feature-enabled package matches if spec matches and feature is required

                        if spec.matches(*rec) {
                            if let Some(spec_feature) = &spec.feature {
                                (*spec_feature == *feature) != inverse
                            } else {
                                inverse
                            }
                        } else {
                            inverse
                        }
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
                }
            })
            .collect()
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
            let name_id = provider.pool.intern_package_name(spec.name.as_normalized());
            provider
                .pool
                .intern_version_set(name_id, NamelessMatchSpec::default().into())
        });

        let root_requirements = task.specs.iter().flat_map(|spec| {
            let (name, nameless_spec) = spec.clone().into_nameless();
            let features = &spec.extras;
            let name = name.expect("cannot use matchspec without a name");
            let name_id = provider.pool.intern_package_name(name.as_normalized());
            let mut reqs = vec![provider
                .pool
                .intern_version_set(name_id, nameless_spec.clone().into())];

            // Add requirements for optional features if specified
            if let Some(features) = features {
                for feature in features {
                    // Create a version set that matches the feature-enabled package
                    let package_name_with_feature = NameType::BaseWithFeature(
                        name.as_normalized().to_owned(),
                        feature.to_string(),
                    );
                    let feature_name_id =
                        provider.pool.intern_package_name(package_name_with_feature);

                    let mut solver_match_spec: SolverMatchSpec<'_> = nameless_spec.clone().into();
                    let _ = solver_match_spec.set_feature(feature.to_string());

                    let feature_version_set = provider
                        .pool
                        .intern_version_set(feature_name_id, solver_match_spec);
                    reqs.push(feature_version_set);
                }
            }
            reqs
        });

        let all_requirements: Vec<Requirement> = virtual_package_requirements
            .chain(root_requirements)
            .map(Requirement::from)
            .collect();

        let root_constraints = task
            .constraints
            .iter()
            .map(|spec| {
                let (name, spec) = spec.clone().into_nameless();
                let name = name.expect("cannot use matchspec without a name");
                let name_id = provider.pool.intern_package_name(name.as_normalized());
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
        let mut features: HashMap<PackageName, Vec<String>> = HashMap::new();
        let mut records = Vec::new();

        for id in solvables {
            match &solver.provider().pool.resolve_solvable(id).record {
                SolverPackageRecord::Record(rec) => {
                    records.push((*rec).clone());
                }
                SolverPackageRecord::RecordWithFeature(rec, feature) => {
                    features
                        .entry(rec.package_record.name.clone())
                        .or_default()
                        .push(feature.clone());
                }
                SolverPackageRecord::VirtualPackage(_) => {}
            }
        }

        Ok(SolverResult { records, features })
    }
}

fn parse_match_spec(
    pool: &Pool<SolverMatchSpec<'_>, NameType>,
    spec_str: &str,
    parse_match_spec_cache: &mut HashMap<String, Vec<VersionSetId>>,
) -> Result<Vec<VersionSetId>, ParseMatchSpecError> {
    if let Some(spec_id) = parse_match_spec_cache.get(spec_str) {
        return Ok(spec_id.clone());
    }

    let match_spec = MatchSpec::from_str(spec_str, ParseStrictness::Lenient)?;
    let (name, spec) = match_spec.into_nameless();

    let mut version_set_ids = vec![];
    if let Some(ref features) = spec.extras {
        let spec_with_feature: SolverMatchSpec<'_> = spec.clone().into();

        for feature in features {
            let name_with_feature = NameType::BaseWithFeature(
                name.as_ref()
                    .expect("Packages with no name are not supported")
                    .as_normalized()
                    .to_owned(),
                feature.to_string(),
            );
            let dependency_name = pool.intern_package_name(name_with_feature);

            let version_set_id = pool.intern_version_set(
                dependency_name,
                spec_with_feature.with_feature(feature.to_string()),
            );
            version_set_ids.push(version_set_id);
        }
    } else {
        let dependency_name = pool.intern_package_name(
            name.as_ref()
                .expect("Packages with no name are not supported")
                .as_normalized(),
        );
        let version_set_id = pool.intern_version_set(dependency_name, spec.into());
        version_set_ids.push(version_set_id);
    }
    parse_match_spec_cache.insert(spec_str.to_string(), version_set_ids.clone());

    Ok(version_set_ids)
}
