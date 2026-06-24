use std::sync::Arc;

use ahash::HashMap;
use rattler_conda_types::{ChannelRelations, PackageName, RepoDataRecord, RepodataRevisions};

use super::GatewayError;
use crate::Reporter;
use crate::sparse::empty_repodata_revisions;
use coalesced_map::{CoalescedGetError, CoalescedMap};

/// Records for a single package, with precomputed unique dependency strings
/// split between unconditional (base) deps and per-extra deps.
///
/// The split lets the gateway walk only the extras that are actually active
/// for a name instead of cascading through every extra's dependencies. For
/// packages without any extras (the common case), `unique_extra_deps` is
/// empty.
#[derive(Clone, Debug, Default)]
pub struct PackageRecords {
    /// All repodata records for this package.
    pub records: Vec<Arc<RepoDataRecord>>,

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
    let mut base_seen = ahash::HashSet::<String>::default();
    let mut base = Vec::new();
    let mut per_extra: HashMap<String, (ahash::HashSet<String>, Vec<String>)> = HashMap::default();

    for record in records {
        for dep in &record.package_record.depends {
            if base_seen.insert(dep.clone()) {
                base.push(dep.clone());
            }
        }
        for (extra, extra_deps) in record.package_record.extra_depends.iter() {
            let entry = per_extra.entry(extra.clone()).or_default();
            for dep in extra_deps {
                if entry.0.insert(dep.clone()) {
                    entry.1.push(dep.clone());
                }
            }
        }
    }

    // Final pass: a dep that ended up in base must not appear in any extra
    // list, regardless of the order records were visited.
    let per_extra: HashMap<String, Arc<[String]>> = per_extra
        .into_iter()
        .filter_map(|(extra, (_, deps))| {
            let filtered: Vec<String> = deps
                .into_iter()
                .filter(|d| !base_seen.contains(d))
                .collect();
            if filtered.is_empty() {
                None
            } else {
                Some((extra, Arc::from(filtered)))
            }
        })
        .collect();

    (Arc::from(base), Arc::new(per_extra))
}

pub enum Subdir {
    /// The subdirectory is missing from the channel, it is considered empty.
    NotFound,

    /// A subdirectory and the data associated with it.
    Found(SubdirData),
}

impl Subdir {
    /// Returns the names of all packages in the subdirectory.
    pub fn package_names(&self) -> Option<Vec<String>> {
        match self {
            Subdir::Found(subdir) => Some(subdir.package_names()),
            Subdir::NotFound => None,
        }
    }

    /// Returns repodata revisions advertised by this subdirectory.
    pub fn repodata_revisions(&self) -> &RepodataRevisions {
        match self {
            Subdir::Found(subdir) => subdir.repodata_revisions(),
            Subdir::NotFound => empty_repodata_revisions(),
        }
    }

    /// [CEP-42] channel relations from this subdir's repodata, or
    /// `None` if absent / subdir not found.
    ///
    /// [CEP-42]: https://github.com/conda/ceps/blob/main/cep-0042.md
    pub fn channel_relations(&self) -> Option<&ChannelRelations> {
        match self {
            Subdir::Found(subdir) => subdir.channel_relations(),
            Subdir::NotFound => None,
        }
    }
}

/// Fetches and caches repodata records by package name for a specific
/// subdirectory of a channel.
pub struct SubdirData {
    /// The client to use to fetch repodata.
    client: Arc<dyn SubdirClient>,

    /// Previously fetched or currently pending records (with precomputed deps).
    records: CoalescedMap<PackageName, PackageRecords>,
}

impl SubdirData {
    pub fn from_client<C: SubdirClient + 'static>(client: C) -> Self {
        Self {
            client: Arc::new(client),
            records: CoalescedMap::new(),
        }
    }

    pub async fn get_or_fetch_package_records(
        &self,
        name: &PackageName,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> Result<PackageRecords, GatewayError> {
        let client = self.client.clone();
        let name_clone = name.clone();

        self.records
            .get_or_try_init(name.clone(), || async move {
                client
                    .fetch_package_records(&name_clone, reporter.as_deref())
                    .await
            })
            .await
            .map_err(|e| match e {
                CoalescedGetError::Init(gateway_err) => gateway_err,
                CoalescedGetError::CoalescedRequestFailed => GatewayError::IoError(
                    "a coalesced request failed".to_string(),
                    std::io::ErrorKind::Other.into(),
                ),
            })
    }

    pub fn package_names(&self) -> Vec<String> {
        self.client.package_names()
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
    /// Fetches all repodata records for the package with the given name in a
    /// channel subdirectory.
    async fn fetch_package_records(
        &self,
        name: &PackageName,
        reporter: Option<&dyn Reporter>,
    ) -> Result<PackageRecords, GatewayError>;

    /// Returns the names of all packages in the subdirectory.
    fn package_names(&self) -> Vec<String>;

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
