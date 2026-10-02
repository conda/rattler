use std::sync::Arc;

use ahash::{HashMap, HashSet};
use dashmap::DashMap;
use rattler_conda_types::{
    ChannelRelations, PackageName, RepoDataRecord, RepodataRevisions,
    package::{ArchiveIdentifier, DistArchiveType},
};
use tokio::sync::Mutex;

use super::GatewayError;
use crate::{
    Reporter,
    sparse::{
        FormatBucket, FormatBucketMap, FormatBucketSet, PackageFormatSelection, RemovedPackage,
        empty_repodata_revisions,
    },
};

/// The records of a single package in one [`PackageFormatSelection`], with
/// precomputed unique dependency strings split between unconditional (base)
/// deps and per-extra deps.
///
/// The split lets the gateway walk only the extras that are actually active
/// for a name instead of cascading through every extra's dependencies. For
/// packages without any extras (the common case), `unique_extra_deps` is
/// empty. Cloning is cheap: every field is reference counted.
#[derive(Clone, Debug, Default)]
pub struct PackageRecords {
    /// The repodata records of this package in the selected formats, sorted
    /// by identifier.
    pub records: Arc<[Arc<RepoDataRecord>]>,

    /// Packages of this name that the subdirectory lists as removed, in any
    /// format. These never appear in `records`.
    pub removed: Arc<[RemovedPackage]>,

    /// Unique base dependency strings across all records.
    pub unique_base_deps: Arc<[String]>,

    /// Unique dependency strings per extra, deduplicated across all records.
    pub unique_extra_deps: ExtraDeps,
}

/// Per-extra deduplicated dependency strings. Empty for packages without any
/// extras (the common case).
pub type ExtraDeps = Arc<HashMap<String, Arc<[String]>>>;

/// Extract the unique dependency strings from a set of records, split into
/// base deps and per-extra deps. Each output list is deduplicated, and a dep
/// that appears in any record's base list is removed from every extra list
/// (a base requirement is unconditional, so the solver does not need it gated
/// on an extra).
pub(crate) fn extract_unique_deps_split<'a>(
    records: impl IntoIterator<Item = &'a RepoDataRecord>,
) -> (Arc<[String]>, ExtraDeps) {
    let mut base_seen = HashSet::<&'a str>::default();
    let mut base = Vec::<&'a str>::new();
    let mut per_extra = HashMap::<&'a str, (HashSet<&'a str>, Vec<&'a str>)>::default();

    for record in records {
        for dep in &record.package_record.depends {
            if base_seen.insert(dep.as_str()) {
                base.push(dep.as_str());
            }
        }
        for (extra, extra_deps) in &record.package_record.extra_depends {
            let (seen, deps) = per_extra.entry(extra.as_str()).or_default();
            for dep in extra_deps {
                if seen.insert(dep.as_str()) {
                    deps.push(dep.as_str());
                }
            }
        }
    }

    // Final pass: a dep that ended up in base must not appear in any extra
    // list, regardless of the order records were visited.
    let per_extra: HashMap<String, Arc<[String]>> = per_extra
        .into_iter()
        .filter_map(|(extra, (_, deps))| {
            let deps: Arc<[String]> = deps
                .into_iter()
                .filter(|dep| !base_seen.contains(dep))
                .map(str::to_owned)
                .collect();
            (!deps.is_empty()).then(|| (extra.to_owned(), deps))
        })
        .collect();

    (
        base.into_iter().map(str::to_owned).collect(),
        Arc::new(per_extra),
    )
}

/// The records of one [`FormatBucket`] of a package.
pub type BucketRecords = Arc<[Arc<RepoDataRecord>]>;

/// The records of a package as returned by a [`SubdirClient`], grouped by
/// [`FormatBucket`].
#[derive(Debug, Default)]
pub struct FetchedPackage {
    /// The records per bucket, or `None` for a bucket that was not loaded.
    buckets: FormatBucketMap<Option<BucketRecords>>,

    /// The packages of this name that the subdirectory lists as removed.
    removed: Arc<[RemovedPackage]>,
}

impl FetchedPackage {
    /// Constructs a package of which only `buckets` were loaded. Records in
    /// other buckets are ignored.
    pub(crate) fn from_buckets(
        buckets: FormatBucketSet,
        records: FormatBucketMap<Vec<Arc<RepoDataRecord>>>,
        removed: Vec<RemovedPackage>,
    ) -> Self {
        let mut loaded = FormatBucketMap::default();
        for (bucket, records) in records {
            if buckets.contains(bucket) {
                loaded[bucket] = Some(records.into());
            }
        }
        Self {
            buckets: loaded,
            removed: removed.into(),
        }
    }

    /// Constructs a package from all of its records, classifying them into
    /// buckets by their identifiers. The records must already exclude
    /// removed packages and contain each file once.
    pub(crate) fn from_records(
        records: Vec<Arc<RepoDataRecord>>,
        removed: Vec<RemovedPackage>,
    ) -> Self {
        let available: HashSet<(&ArchiveIdentifier, DistArchiveType)> = records
            .iter()
            .map(|record| {
                (
                    &record.identifier.identifier,
                    record.identifier.archive_type,
                )
            })
            .collect();
        let record_buckets: Vec<FormatBucket> = records
            .iter()
            .map(|record| {
                FormatBucket::classify(record.identifier.archive_type, |twin| {
                    available.contains(&(&record.identifier.identifier, twin))
                })
            })
            .collect();
        drop(available);

        let mut buckets = FormatBucketMap::<Vec<Arc<RepoDataRecord>>>::default();
        for (bucket, record) in record_buckets.into_iter().zip(records) {
            buckets[bucket].push(record);
        }
        Self::from_buckets(FormatBucketSet::ALL, buckets, removed)
    }

    /// Constructs a package without any records or removed packages, with all
    /// buckets loaded.
    pub(crate) fn empty() -> Self {
        Self::from_buckets(FormatBucketSet::ALL, FormatBucketMap::default(), Vec::new())
    }

    /// The records of every loaded bucket, with the bucket they belong to.
    #[cfg(test)]
    pub(crate) fn records(&self) -> impl Iterator<Item = (FormatBucket, &Arc<RepoDataRecord>)> {
        FormatBucket::ALL.into_iter().flat_map(|bucket| {
            self.buckets[bucket]
                .iter()
                .flat_map(move |records| records.iter().map(move |record| (bucket, record)))
        })
    }

    /// The packages of this name that the subdirectory lists as removed.
    #[cfg(test)]
    pub(crate) fn removed(&self) -> &[RemovedPackage] {
        &self.removed
    }
}

pub enum SubdirState {
    /// The subdirectory is missing from the channel, it is considered empty.
    NotFound,

    /// A subdirectory and the data associated with it.
    Found(SubdirData),
}

impl SubdirState {
    /// Returns the names of the packages in the subdirectory that may have
    /// records in `selection`, see [`SubdirClient::package_names`].
    pub fn package_names(&self, selection: PackageFormatSelection) -> Option<Vec<String>> {
        match self {
            SubdirState::Found(subdir) => Some(subdir.package_names(selection)),
            SubdirState::NotFound => None,
        }
    }

    /// Returns repodata revisions advertised by this subdirectory.
    pub fn repodata_revisions(&self) -> &RepodataRevisions {
        match self {
            SubdirState::Found(subdir) => subdir.repodata_revisions(),
            SubdirState::NotFound => empty_repodata_revisions(),
        }
    }

    /// [CEP-42] channel relations from this subdir's repodata, or
    /// `None` if absent / subdir not found.
    ///
    /// [CEP-42]: https://github.com/conda/ceps/blob/main/cep-0042.md
    pub fn channel_relations(&self) -> Option<&ChannelRelations> {
        match self {
            SubdirState::Found(subdir) => subdir.channel_relations(),
            SubdirState::NotFound => None,
        }
    }
}

/// Fetches and caches repodata records by package name for a specific
/// subdirectory of a channel.
///
/// This is the only place records are cached. The records of a package are
/// fetched per [`FormatBucket`]: a bucket is fetched at most once, and every
/// [`PackageFormatSelection`] requested for the package is served from the
/// same record allocations.
pub struct SubdirData {
    /// The client to use to fetch repodata.
    client: Arc<dyn SubdirClient>,

    /// What is known about each package name requested so far. The state of
    /// a name stays locked while its records are fetched, so concurrent
    /// requests for the same name wait for a single fetch.
    packages: DashMap<PackageName, Arc<Mutex<PackageState>>, ahash::RandomState>,
}

/// The records of one package name loaded so far.
#[derive(Default)]
struct PackageState {
    /// The records of each bucket that was fetched successfully.
    buckets: FormatBucketMap<Option<BucketRecords>>,

    /// The removed packages of this name, set by every successful fetch.
    removed: Arc<[RemovedPackage]>,

    /// The records of every selection requested so far.
    selections: SelectionRecords,
}

impl PackageState {
    /// Returns the buckets of `buckets` that were not fetched yet.
    fn missing_buckets(&self, buckets: FormatBucketSet) -> FormatBucketSet {
        buckets
            .iter()
            .filter(|bucket| self.buckets[*bucket].is_none())
            .collect()
    }

    /// Stores the buckets of `fetched` that are not loaded yet. Loaded
    /// buckets are kept so the records of earlier selections stay shared.
    fn store(&mut self, fetched: FetchedPackage) {
        for (bucket, records) in fetched.buckets {
            if self.buckets[bucket].is_none() {
                self.buckets[bucket] = records;
            }
        }
        self.removed = fetched.removed;
    }

    /// Builds the records of `selection` from the loaded buckets.
    fn select(&self, selection: PackageFormatSelection) -> PackageRecords {
        let mut records: Vec<Arc<RepoDataRecord>> = selection
            .buckets()
            .iter()
            .filter_map(|bucket| self.buckets[bucket].as_deref())
            .flatten()
            .cloned()
            .collect();
        records.sort_unstable_by(|a, b| a.identifier.cmp(&b.identifier));
        let (unique_base_deps, unique_extra_deps) =
            extract_unique_deps_split(records.iter().map(AsRef::as_ref));
        PackageRecords {
            records: records.into(),
            removed: self.removed.clone(),
            unique_base_deps,
            unique_extra_deps,
        }
    }
}

/// The records of a package per [`PackageFormatSelection`], each built once
/// from the loaded buckets.
#[derive(Default)]
struct SelectionRecords {
    only_tar_bz2: Option<PackageRecords>,
    only_conda: Option<PackageRecords>,
    prefer_conda: Option<PackageRecords>,
    prefer_conda_with_whl: Option<PackageRecords>,
    both: Option<PackageRecords>,
    all: Option<PackageRecords>,
}

impl SelectionRecords {
    fn get(&self, selection: PackageFormatSelection) -> Option<&PackageRecords> {
        match selection {
            PackageFormatSelection::OnlyTarBz2 => self.only_tar_bz2.as_ref(),
            PackageFormatSelection::OnlyConda => self.only_conda.as_ref(),
            PackageFormatSelection::PreferConda => self.prefer_conda.as_ref(),
            PackageFormatSelection::PreferCondaWithWhl => self.prefer_conda_with_whl.as_ref(),
            PackageFormatSelection::Both => self.both.as_ref(),
            PackageFormatSelection::All => self.all.as_ref(),
        }
    }

    fn insert(&mut self, selection: PackageFormatSelection, records: PackageRecords) {
        let slot = match selection {
            PackageFormatSelection::OnlyTarBz2 => &mut self.only_tar_bz2,
            PackageFormatSelection::OnlyConda => &mut self.only_conda,
            PackageFormatSelection::PreferConda => &mut self.prefer_conda,
            PackageFormatSelection::PreferCondaWithWhl => &mut self.prefer_conda_with_whl,
            PackageFormatSelection::Both => &mut self.both,
            PackageFormatSelection::All => &mut self.all,
        };
        *slot = Some(records);
    }
}

impl SubdirData {
    pub fn from_client<C: SubdirClient + 'static>(client: C) -> Self {
        Self {
            client: Arc::new(client),
            packages: DashMap::default(),
        }
    }

    /// Returns the state of `name`, creating an empty one if the name was
    /// not requested before.
    fn package_state(&self, name: &PackageName) -> Arc<Mutex<PackageState>> {
        if let Some(state) = self.packages.get(name) {
            return state.clone();
        }
        self.packages.entry(name.clone()).or_default().clone()
    }

    /// Returns the records of `name` in the given package format selection,
    /// fetching the buckets that were not loaded before. Errors of the client
    /// are returned as-is and leave the cache unchanged.
    pub async fn get_or_fetch_package_records(
        &self,
        name: &PackageName,
        selection: PackageFormatSelection,
        reporter: Option<&dyn Reporter>,
    ) -> Result<PackageRecords, GatewayError> {
        let state = self.package_state(name);
        let mut state = state.lock().await;
        if let Some(records) = state.selections.get(selection) {
            return Ok(records.clone());
        }

        let missing = state.missing_buckets(selection.buckets());
        if !missing.is_empty() {
            let fetched = self
                .client
                .fetch_package_records(name, missing, reporter)
                .await?;
            state.store(fetched);
        }

        let records = state.select(selection);
        state.selections.insert(selection, records.clone());
        Ok(records)
    }

    /// Returns the records of `name` in the default package format selection
    /// without caching anything. Buckets that are already loaded are reused,
    /// the others are fetched and dropped afterwards. Used by streaming scans
    /// (e.g. the gateway's `who_needs` query) that visit every package of a
    /// subdir exactly once and would otherwise permanently fill the cache
    /// with millions of records.
    pub async fn scan_package_records(
        &self,
        name: &PackageName,
        reporter: Option<&dyn Reporter>,
    ) -> Result<Vec<Arc<RepoDataRecord>>, GatewayError> {
        let selection = PackageFormatSelection::default();
        let mut loaded = FormatBucketMap::<Option<BucketRecords>>::default();
        let state = self.packages.get(name).map(|state| state.clone());
        if let Some(state) = state {
            let state = state.lock().await;
            if let Some(records) = state.selections.get(selection) {
                return Ok(records.records.to_vec());
            }
            for bucket in selection.buckets().iter() {
                loaded[bucket].clone_from(&state.buckets[bucket]);
            }
        }

        let missing: FormatBucketSet = selection
            .buckets()
            .iter()
            .filter(|bucket| loaded[*bucket].is_none())
            .collect();
        let fetched = if missing.is_empty() {
            FetchedPackage::default()
        } else {
            self.client
                .fetch_package_records(name, missing, reporter)
                .await?
        };

        Ok(selection
            .buckets()
            .iter()
            .filter_map(|bucket| {
                loaded[bucket]
                    .as_deref()
                    .or(fetched.buckets[bucket].as_deref())
            })
            .flatten()
            .cloned()
            .collect())
    }

    /// The number of package names currently held in the per-name record
    /// cache.
    #[cfg(test)]
    pub(crate) fn cached_package_count(&self) -> usize {
        self.packages.len()
    }

    /// Returns the names of the packages that may have records in
    /// `selection`, see [`SubdirClient::package_names`].
    pub fn package_names(&self, selection: PackageFormatSelection) -> Vec<String> {
        self.client.package_names(selection)
    }

    pub fn repodata_revisions(&self) -> &RepodataRevisions {
        self.client.repodata_revisions()
    }

    /// [CEP-42] channel relations from this subdir's repodata, if any.
    ///
    /// [CEP-42]: https://github.com/conda/ceps/blob/main/cep-0042.md
    pub fn channel_relations(&self) -> Option<&ChannelRelations> {
        self.client.channel_relations()
    }
}

/// A client that can be used to fetch repodata for a specific subdirectory.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait SubdirClient: Send + Sync {
    /// Fetches the repodata records for the package with the given name in a
    /// channel subdirectory, grouped by [`FormatBucket`].
    ///
    /// The result contains at least the records of `buckets`. A client that
    /// has to parse every record of the package anyway returns all buckets.
    /// Clients do not cache records; [`SubdirData`] does.
    async fn fetch_package_records(
        &self,
        name: &PackageName,
        buckets: FormatBucketSet,
        reporter: Option<&dyn Reporter>,
    ) -> Result<FetchedPackage, GatewayError>;

    /// Returns the names of the packages in the subdirectory that may have
    /// records in `selection`.
    ///
    /// A client whose index tells which archive formats each package has
    /// returns exactly the names with records in `selection`. A client that
    /// only knows names (sharded repodata, custom sources) returns every name,
    /// because filtering would mean fetching every package.
    fn package_names(&self, selection: PackageFormatSelection) -> Vec<String>;

    /// Returns repodata revisions advertised by the subdirectory.
    fn repodata_revisions(&self) -> &RepodataRevisions {
        empty_repodata_revisions()
    }

    /// [CEP-42] channel relations from this subdir's repodata, if any.
    /// Sources without CEP-42 metadata (e.g. custom) keep the default.
    ///
    /// [CEP-42]: https://github.com/conda/ceps/blob/main/cep-0042.md
    fn channel_relations(&self) -> Option<&ChannelRelations> {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::str::FromStr;

    use rattler_conda_types::{
        NoArchType, PackageRecord, RepoDataRecord, VersionWithSource,
        package::DistArchiveIdentifier,
    };
    use url::Url;

    use super::extract_unique_deps_split;

    fn make_record(name: &str, deps: &[&str], extra_deps: &[(&str, &[&str])]) -> RepoDataRecord {
        let mut extra_depends: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (extra, items) in extra_deps {
            extra_depends.insert(
                (*extra).to_string(),
                items.iter().map(|s| (*s).to_string()).collect(),
            );
        }

        let package_record = PackageRecord {
            attestations_sha256: None,
            arch: None,
            build: "0".to_string(),
            build_number: 0,
            constrains: Vec::new(),
            depends: deps.iter().map(|s| (*s).to_string()).collect(),
            features: None,
            flags: Vec::new(),
            legacy_bz2_md5: None,
            legacy_bz2_size: None,
            license: None,
            license_family: None,
            md5: None,
            name: name.parse().unwrap(),
            noarch: NoArchType::default(),
            platform: None,
            python_site_packages_path: None,
            extra_depends,
            sha256: None,
            size: None,
            subdir: "linux-64".to_string(),
            timestamp: None,
            indexed_timestamp: None,
            track_features: Vec::new(),
            version: VersionWithSource::from_str("1.0").unwrap(),
            purls: None,
            run_exports: None,
        };

        RepoDataRecord {
            url: Url::parse(&format!("https://example.com/{name}-1.0-0.conda")).unwrap(),
            channel: None,
            package_record,
            identifier: format!("{name}-1.0-0.conda")
                .parse::<DistArchiveIdentifier>()
                .unwrap(),
        }
    }

    #[test]
    fn extract_unique_deps_split_base_only() {
        let rec = make_record("foo", &["bar >=1", "baz"], &[]);
        let (base, per_extra) = extract_unique_deps_split([&rec]);
        assert_eq!(&*base, &["bar >=1".to_string(), "baz".to_string()]);
        assert!(per_extra.is_empty());
    }

    #[test]
    fn extract_unique_deps_split_dedupes_across_records() {
        let rec_a = make_record("foo", &["bar >=1", "baz"], &[]);
        let rec_b = make_record("foo", &["bar >=1", "qux"], &[]);
        let (base, per_extra) = extract_unique_deps_split([&rec_a, &rec_b]);
        assert_eq!(
            &*base,
            &["bar >=1".to_string(), "baz".to_string(), "qux".to_string()]
        );
        assert!(per_extra.is_empty());
    }

    #[test]
    fn extract_unique_deps_split_per_extra() {
        let rec = make_record(
            "black",
            &["click >=8"],
            &[
                ("d", &["aiohttp >=3"]),
                ("jupyter", &["ipython", "qtconsole"]),
            ],
        );
        let (base, per_extra) = extract_unique_deps_split([&rec]);
        assert_eq!(&*base, &["click >=8".to_string()]);
        assert_eq!(per_extra.len(), 2);
        assert_eq!(&*per_extra["d"], &["aiohttp >=3".to_string()]);
        assert_eq!(
            &*per_extra["jupyter"],
            &["ipython".to_string(), "qtconsole".to_string()]
        );
    }

    #[test]
    fn extract_unique_deps_split_skips_extra_dep_already_in_base() {
        let rec = make_record("black", &["aiohttp"], &[("d", &["aiohttp", "aiosignal"])]);
        let (base, per_extra) = extract_unique_deps_split([&rec]);
        assert_eq!(&*base, &["aiohttp".to_string()]);
        assert_eq!(&*per_extra["d"], &["aiosignal".to_string()]);
    }

    /// A dep that appears in the base set of one record and in an extra of
    /// another must not be repeated in the extra (base wins).
    #[test]
    fn extract_unique_deps_split_base_wins_across_records() {
        let rec_a = make_record("black", &["aiohttp"], &[]);
        let rec_b = make_record("black", &[], &[("d", &["aiohttp", "aiosignal"])]);
        let (base, per_extra) = extract_unique_deps_split([&rec_a, &rec_b]);
        assert_eq!(&*base, &["aiohttp".to_string()]);
        assert_eq!(&*per_extra["d"], &["aiosignal".to_string()]);
    }

    /// Same as `extract_unique_deps_split_base_wins_across_records` but with
    /// the records visited in the opposite order. The base-wins invariant
    /// must hold regardless of iteration order.
    #[test]
    fn extract_unique_deps_split_base_wins_reversed_order() {
        let rec_extra_first = make_record("black", &[], &[("d", &["aiohttp", "aiosignal"])]);
        let rec_base_after = make_record("black", &["aiohttp"], &[]);
        let (base, per_extra) = extract_unique_deps_split([&rec_extra_first, &rec_base_after]);
        assert_eq!(&*base, &["aiohttp".to_string()]);
        assert_eq!(&*per_extra["d"], &["aiosignal".to_string()]);
    }

    /// An extra whose only dep also appears in some record's base list must
    /// not produce an empty entry in the per-extra map.
    #[test]
    fn extract_unique_deps_split_extra_fully_subsumed_is_dropped() {
        let rec_extra_first = make_record("black", &[], &[("d", &["aiohttp"])]);
        let rec_base_after = make_record("black", &["aiohttp"], &[]);
        let (base, per_extra) = extract_unique_deps_split([&rec_extra_first, &rec_base_after]);
        assert_eq!(&*base, &["aiohttp".to_string()]);
        assert!(per_extra.is_empty());
    }

    #[test]
    fn extract_unique_deps_split_empty_records() {
        let records: Vec<&RepoDataRecord> = Vec::new();
        let (base, per_extra) = extract_unique_deps_split(records);
        assert!(base.is_empty());
        assert!(per_extra.is_empty());
    }
}
