//! Records the solver favors (`--favor`) or pins (`--pin`).
//!
//! A source is a lock file, a prefix, a single `conda-meta` record or a
//! package archive. Every source resolves to a list of [`RepoDataRecord`]s,
//! which end up in [`rattler_solve::SolverTask::locked_packages`] or
//! [`rattler_solve::SolverTask::pinned_packages`].

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    convert::Infallible,
    fmt,
    io::Cursor,
    path::PathBuf,
    str::FromStr,
};

use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{
    MatchSpec, PackageName, ParseMatchSpecOptions, PrefixRecord, RepoDataRecord, Subdir,
    package::{CondaArchiveIdentifier, CondaArchiveType, DistArchiveIdentifier},
};
use rattler_lock::{LockFile, UrlOrPath};
use rattler_repodata_gateway::RepoData;
use reqwest_middleware::ClientWithMiddleware;
use url::Url;

use crate::commands::package_source::PackageSource;

/// A `--favor` or `--pin` value.
#[derive(Debug, Clone)]
pub enum RecordSource {
    /// A remote package archive.
    Url(Url),
    /// A prefix, whose `conda-meta` records are used.
    Prefix(PathBuf),
    /// A local package archive.
    Archive(PathBuf),
    /// A single `conda-meta/*.json` record.
    PrefixRecord(PathBuf),
    /// A lock file such as `pixi.lock`.
    LockFile(PathBuf),
}

impl FromStr for RecordSource {
    type Err = Infallible;

    /// Every value is a valid source. Whether it can be read is only known
    /// once it is loaded. Lock files are recognized by their content rather
    /// than their name, so anything that is not obviously something else is
    /// treated as one.
    fn from_str(source: &str) -> Result<Self, Self::Err> {
        Ok(match PackageSource::parse(source) {
            PackageSource::Url(url) => Self::Url(url),
            PackageSource::Path(path) if path.is_dir() => Self::Prefix(path),
            PackageSource::Path(path) if CondaArchiveType::try_from(&path).is_some() => {
                Self::Archive(path)
            }
            PackageSource::Path(path) if path.extension().is_some_and(|ext| ext == "json") => {
                Self::PrefixRecord(path)
            }
            PackageSource::Path(path) => Self::LockFile(path),
        })
    }
}

impl fmt::Display for RecordSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Url(url) => write!(f, "{url}"),
            Self::Prefix(path)
            | Self::Archive(path)
            | Self::PrefixRecord(path)
            | Self::LockFile(path) => write!(f, "{}", path.display()),
        }
    }
}

/// Whether the records of a source are favored or pinned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Favor,
    Pin,
}

impl Mode {
    fn verb(self) -> &'static str {
        match self {
            Self::Favor => "Favoring",
            Self::Pin => "Pinning",
        }
    }
}

/// The records of a single source.
#[derive(Debug)]
enum Slot {
    /// Records read from a local source.
    Records(Vec<RepoDataRecord>),
    /// A remote archive. It is only downloaded when it is not part of the
    /// loaded repodata, see [`SelectedRecords::resolve_remote`].
    Remote {
        url: Url,
        name: PackageName,
        record: Option<Box<RepoDataRecord>>,
    },
}

/// The records to favor and pin, in the order their sources were given.
#[derive(Debug, Default)]
pub struct SelectedRecords {
    favor: Vec<Slot>,
    pin: Vec<Slot>,
}

/// Options that select the records of a lock file.
pub struct LockFileSelection<'a> {
    /// The environment to read.
    pub environment: &'a str,
    /// The platform to read the records of.
    pub platform: Subdir,
}

impl SelectedRecords {
    /// Reads the local sources. Remote archives are resolved later by
    /// [`Self::resolve_remote`], once the repodata is loaded.
    pub fn load(
        favor: &[RecordSource],
        pin: &[RecordSource],
        lock_file: &LockFileSelection<'_>,
    ) -> miette::Result<Self> {
        let mut selected = Self::default();
        for source in favor {
            let slot = load_source(source, lock_file, Mode::Favor)?;
            selected.favor.push(slot);
        }
        for source in pin {
            let slot = load_source(source, lock_file, Mode::Pin)?;
            selected.pin.push(slot);
        }
        Ok(selected)
    }

    /// Favors `records` after the records of all other sources, so those win
    /// when they disagree.
    pub fn favor_after(&mut self, records: Vec<RepoDataRecord>) {
        self.favor.push(Slot::Records(records));
    }

    /// Whether no records are favored or pinned.
    pub fn is_empty(&self) -> bool {
        self.favor.is_empty() && self.pin.is_empty()
    }

    /// The package names the repodata query has to cover on top of the specs.
    ///
    /// These are the dependencies of the selected records: a favored record
    /// is typically an older build whose dependencies the current versions in
    /// the channels no longer need, so the query would not load them
    /// otherwise. Remote archives add their own name, so they can be looked up
    /// in the repodata instead of being downloaded.
    pub fn query_names(&self) -> BTreeSet<PackageName> {
        let mut names = BTreeSet::new();
        for slot in self.favor.iter().chain(&self.pin) {
            match slot {
                Slot::Records(records) => {
                    names.extend(records.iter().flat_map(dependency_names));
                }
                Slot::Remote { name, record, .. } => {
                    names.insert(name.clone());
                    names.extend(record.iter().flat_map(|record| dependency_names(record)));
                }
            }
        }
        names
    }

    /// Resolves the remote archives that are not part of `repo_data` by
    /// downloading them.
    ///
    /// Returns whether [`Self::query_names`] changed, in which case the
    /// repodata has to be queried again to cover the dependencies of the
    /// downloaded records.
    pub async fn resolve_remote(
        &mut self,
        repo_data: &[RepoData],
        client: &ClientWithMiddleware,
    ) -> miette::Result<bool> {
        let remote: HashSet<Url> = self
            .favor
            .iter()
            .chain(&self.pin)
            .filter_map(|slot| match slot {
                Slot::Remote {
                    url, record: None, ..
                } => Some(url.clone()),
                _ => None,
            })
            .collect();
        if remote.is_empty() {
            return Ok(false);
        }
        let known: HashSet<&Url> = repo_data
            .iter()
            .flat_map(RepoData::iter)
            .map(|record| &record.url)
            .filter(|url| remote.contains(*url))
            .collect();

        let before = self.query_names();
        for slot in self.favor.iter_mut().chain(&mut self.pin) {
            if let Slot::Remote {
                url,
                record: record @ None,
                ..
            } = slot
                && !known.contains(url)
            {
                *record = Some(Box::new(download_record(url, client).await?));
            }
        }
        Ok(self.query_names() != before)
    }

    /// Returns the records to favor and to pin.
    ///
    /// A record whose URL is part of `repo_data` is replaced by the record
    /// from the repodata, which carries the current repodata patches. Records
    /// are deduplicated by URL, the first occurrence wins, and a pin wins over
    /// a favor of the same package.
    pub fn finish(
        self,
        repo_data: &[RepoData],
    ) -> miette::Result<(Vec<RepoDataRecord>, Vec<RepoDataRecord>)> {
        if self.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }

        let channel_records = channel_records(repo_data);
        let flatten = |slots: Vec<Slot>| {
            let mut seen = HashSet::new();
            let mut records = Vec::new();
            for slot in slots {
                let slot_records = match slot {
                    Slot::Records(records) => records,
                    Slot::Remote { url, record, .. } => match record {
                        Some(record) => vec![*record],
                        None => channel_records
                            .get(&url)
                            .map(|&record| vec![record.clone()])
                            .unwrap_or_default(),
                    },
                };
                for record in slot_records {
                    let record = channel_records
                        .get(&record.url)
                        .map_or(record, |&channel_record| channel_record.clone());
                    if seen.insert(record.url.clone()) {
                        records.push(record);
                    }
                }
            }
            records
        };

        let pinned = flatten(self.pin);
        let mut pinned_names: HashMap<&PackageName, &RepoDataRecord> = HashMap::new();
        for record in &pinned {
            if let Some(other) = pinned_names.insert(&record.package_record.name, record) {
                return Err(miette::miette!(
                    "'{}' is pinned to two different packages: {} and {}",
                    record.package_record.name.as_source(),
                    other.url,
                    record.url
                ));
            }
        }

        let mut favored_names: HashMap<PackageName, Url> = HashMap::new();
        let favored = flatten(self.favor)
            .into_iter()
            .filter(|record| !pinned_names.contains_key(&record.package_record.name))
            .filter(|record| {
                if let Some(first) = favored_names.get(&record.package_record.name) {
                    eprintln!(
                        "warning: '{}' is favored more than once, ignoring {} in favor of {first}",
                        record.package_record.name.as_source(),
                        record.url,
                    );
                    return false;
                }
                favored_names.insert(record.package_record.name.clone(), record.url.clone());
                true
            })
            .collect::<Vec<_>>();

        for record in &favored {
            tracing::debug!("favoring {}", record.url);
        }
        for record in &pinned {
            tracing::debug!("pinning {}", record.url);
        }

        Ok((favored, pinned))
    }
}

/// The records in `repo_data` by URL.
fn channel_records(repo_data: &[RepoData]) -> HashMap<&Url, &RepoDataRecord> {
    repo_data
        .iter()
        .flat_map(RepoData::iter)
        .map(|record| (&record.url, record))
        .collect()
}

/// The names of the packages `record` depends on.
fn dependency_names(record: &RepoDataRecord) -> impl Iterator<Item = PackageName> + '_ {
    record.package_record.depends.iter().filter_map(|depends| {
        MatchSpec::from_str(depends, ParseMatchSpecOptions::lenient())
            .ok()?
            .name
            .into_exact()
    })
}

/// Reads the records of a local source and reports what it found.
fn load_source(
    source: &RecordSource,
    lock_file: &LockFileSelection<'_>,
    mode: Mode,
) -> miette::Result<Slot> {
    let verb = mode.verb();
    let records = match source {
        RecordSource::Url(url) => {
            let identifier = CondaArchiveIdentifier::try_from_url(url).ok_or_else(|| {
                miette::miette!(
                    "{url} is not a package archive, only .conda and .tar.bz2 URLs are supported"
                )
            })?;
            let name = PackageName::from_str(&identifier.identifier.name)
                .into_diagnostic()
                .with_context(|| format!("invalid package name in {url}"))?;
            eprintln!("{verb} {url}");
            return Ok(Slot::Remote {
                url: url.clone(),
                name,
                record: None,
            });
        }
        RecordSource::Prefix(path) => {
            if !path.join("conda-meta").is_dir() {
                return Err(miette::miette!(
                    "{} is not a prefix, it has no conda-meta directory",
                    path.display()
                ));
            }
            let records = PrefixRecord::collect_from_prefix::<PrefixRecord>(path)
                .into_diagnostic()
                .with_context(|| format!("failed to read the prefix {}", path.display()))?;
            let records: Vec<_> = records
                .into_iter()
                .map(|record| record.repodata_record)
                .collect();
            eprintln!(
                "{verb} {} from the prefix {}",
                packages(records.len()),
                path.display()
            );
            records
        }
        RecordSource::PrefixRecord(path) => {
            let record = PrefixRecord::from_path(path)
                .into_diagnostic()
                .with_context(|| format!("failed to read {} as a prefix record", path.display()))?
                .repodata_record;
            eprintln!("{verb} {}", record.url);
            vec![record]
        }
        RecordSource::Archive(path) => {
            let record = archive_record(path)?;
            eprintln!("{verb} {}", path.display());
            vec![record]
        }
        RecordSource::LockFile(path) => {
            let (records, platform) = lock_file_records(path, lock_file)?;
            eprintln!(
                "{verb} {} from {} (environment '{}', {platform})",
                packages(records.len()),
                path.display(),
                lock_file.environment
            );
            records
        }
    };
    Ok(Slot::Records(records))
}

/// `n packages`, with the right plural.
fn packages(n: usize) -> String {
    format!("{n} package{}", if n == 1 { "" } else { "s" })
}

/// Builds the record of a local package archive from its `index.json`.
fn archive_record(path: &std::path::Path) -> miette::Result<RepoDataRecord> {
    let path = dunce::canonicalize(path)
        .into_diagnostic()
        .with_context(|| format!("failed to read {}", path.display()))?;
    let identifier = DistArchiveIdentifier::try_from_path(&path).ok_or_else(|| {
        miette::miette!(
            "{} is not named like a package archive (name-version-build.conda)",
            path.display()
        )
    })?;
    let package_record = rattler_index::package_record_from_archive(&path)
        .into_diagnostic()
        .with_context(|| format!("failed to read the package {}", path.display()))?;
    let url = Url::from_file_path(&path)
        .map_err(|()| miette::miette!("{} cannot be converted to a URL", path.display()))?;
    Ok(RepoDataRecord {
        package_record,
        identifier,
        url,
        channel: None,
    })
}

/// Downloads a remote package archive and builds its record from its
/// `index.json`.
async fn download_record(
    url: &Url,
    client: &ClientWithMiddleware,
) -> miette::Result<RepoDataRecord> {
    let identifier = CondaArchiveIdentifier::try_from_url(url)
        .ok_or_else(|| miette::miette!("{url} is not a package archive"))?;
    let bytes = client
        .get(url.clone())
        .send()
        .await
        .into_diagnostic()
        .and_then(|response| response.error_for_status().into_diagnostic())
        .with_context(|| format!("failed to download {url}"))?
        .bytes()
        .await
        .into_diagnostic()
        .with_context(|| format!("failed to download {url}"))?;
    let reader = Cursor::new(bytes);
    let package_record = match identifier.archive_type {
        CondaArchiveType::Conda => rattler_index::package_record_from_conda_reader(reader),
        CondaArchiveType::TarBz2 => rattler_index::package_record_from_tar_bz2_reader(reader),
    }
    .into_diagnostic()
    .with_context(|| format!("failed to read the package {url}"))?;
    Ok(RepoDataRecord {
        package_record,
        identifier: identifier.into(),
        url: url.clone(),
        channel: None,
    })
}

/// Reads the conda records of the selected environment and platform of a lock
/// file. Returns the name of the platform that was read too.
fn lock_file_records(
    path: &std::path::Path,
    selection: &LockFileSelection<'_>,
) -> miette::Result<(Vec<RepoDataRecord>, String)> {
    let lock_file = LockFile::from_path(path)
        .into_diagnostic()
        .with_context(|| format!("failed to read {} as a lock file", path.display()))?;

    let environment = lock_file
        .environment(selection.environment)
        .ok_or_else(|| {
            let available = lock_file
                .environments()
                .map(|(name, _)| format!("'{name}'"))
                .sorted()
                .collect::<Vec<_>>();
            miette::miette!(
                "{} has no environment '{}' (available: {})",
                path.display(),
                selection.environment,
                if available.is_empty() {
                    "none".to_string()
                } else {
                    available.join(", ")
                }
            )
        })?;

    // A platform is usually named after its subdir. Lock files can also name
    // platforms freely, so fall back to the one platform with that subdir.
    let subdir = selection.platform;
    let platform = environment
        .platforms()
        .find(|platform| platform.name().to_string() == subdir.as_str())
        .or_else(|| {
            let mut matching = environment
                .platforms()
                .filter(|platform| platform.subdir() == subdir);
            let first = matching.next()?;
            matching.next().is_none().then_some(first)
        })
        .ok_or_else(|| {
            let available = environment
                .platforms()
                .map(|platform| platform.name().to_string())
                .collect::<Vec<_>>();
            miette::miette!(
                "environment '{}' in {} has no packages for {subdir} (available: {}), \
                 select the platform with --platform",
                selection.environment,
                path.display(),
                if available.is_empty() {
                    "none".to_string()
                } else {
                    available.join(", ")
                }
            )
        })?;

    let source_packages = environment
        .conda_packages(platform)
        .into_iter()
        .flatten()
        .filter(|package| package.as_source().is_some())
        .count();
    if source_packages > 0 {
        eprintln!(
            "warning: ignoring {} in {} that {} built from source",
            packages(source_packages),
            path.display(),
            if source_packages == 1 { "is" } else { "are" }
        );
    }

    // Paths in a lock file are relative to the directory of the lock file.
    let lock_file_dir = std::path::absolute(path)
        .into_diagnostic()?
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let records = environment
        .conda_packages(platform)
        .into_iter()
        .flatten()
        .filter_map(|package| package.as_binary())
        .map(|package| {
            let url = match &package.location {
                UrlOrPath::Url(url) => url.clone(),
                UrlOrPath::Path(location) => {
                    let location = lock_file_dir.join(location.as_str());
                    std::path::absolute(&location)
                        .ok()
                        .and_then(|location| Url::from_file_path(location).ok())
                        .ok_or_else(|| {
                            miette::miette!(
                                "{} in {} cannot be converted to a URL",
                                location.display(),
                                path.display()
                            )
                        })?
                }
            };
            Ok(RepoDataRecord {
                package_record: package.package_record.clone(),
                identifier: package.file_name.clone(),
                url,
                channel: package.channel.as_ref().map(ToString::to_string),
            })
        })
        .collect::<miette::Result<Vec<_>>>()?;
    Ok((records, platform.name().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKSPACE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

    fn test_data(path: &str) -> PathBuf {
        PathBuf::from(WORKSPACE_ROOT).join("test-data").join(path)
    }

    fn source(path: &str) -> RecordSource {
        RecordSource::from_str(test_data(path).to_str().unwrap()).unwrap()
    }

    fn selection(environment: &str, platform: Subdir) -> LockFileSelection<'_> {
        LockFileSelection {
            environment,
            platform,
        }
    }

    #[test]
    fn test_parse_sources() {
        assert!(matches!(
            RecordSource::from_str("https://example.com/foo-1.0-0.conda").unwrap(),
            RecordSource::Url(_)
        ));
        assert!(matches!(source(""), RecordSource::Prefix(_)));
        assert!(matches!(
            source("packages/empty-0.1.0-h4616a5c_0.conda"),
            RecordSource::Archive(_)
        ));
        assert!(matches!(
            source("conda-meta/tk-8.6.13-h5083fa2_1.json"),
            RecordSource::PrefixRecord(_)
        ));
        assert!(matches!(
            source("conda-lock/v6/options-lock.yml"),
            RecordSource::LockFile(_)
        ));
        assert!(matches!(
            RecordSource::from_str("pixi.lock").unwrap(),
            RecordSource::LockFile(_)
        ));
    }

    #[test]
    fn test_load_prefix_and_prefix_record() {
        let selection = selection("default", Subdir::Linux64);
        let selected = SelectedRecords::load(
            &[source("")],
            &[source("conda-meta/tk-8.6.13-h5083fa2_1.json")],
            &selection,
        )
        .unwrap();
        let (favored, pinned) = selected.finish(&[]).unwrap();
        assert!(!favored.is_empty());
        assert!(
            favored
                .iter()
                .all(|record| record.package_record.name.as_normalized() != "tk"),
            "a pinned package is not favored too"
        );
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].package_record.name.as_normalized(), "tk");
    }

    #[test]
    fn test_load_archive() {
        let selection = selection("default", Subdir::Linux64);
        let selected = SelectedRecords::load(
            &[source("packages/empty-0.1.0-h4616a5c_0.conda")],
            &[],
            &selection,
        )
        .unwrap();
        let (favored, _) = selected.finish(&[]).unwrap();
        let [record] = favored.as_slice() else {
            panic!("expected one record, got {favored:?}");
        };
        assert_eq!(record.package_record.name.as_normalized(), "empty");
        assert_eq!(record.url.scheme(), "file");
        assert_eq!(
            record.identifier.to_file_name(),
            "empty-0.1.0-h4616a5c_0.conda"
        );
        assert!(record.package_record.sha256.is_some());
        assert!(record.package_record.md5.is_some());
        assert!(record.package_record.size.is_some());
    }

    #[test]
    fn test_load_lock_file() {
        let lock_file = "channels/favor/pixi.lock";
        let load = |environment: &str, platform: Subdir| {
            let selection = selection(environment, platform);
            SelectedRecords::load(&[source(lock_file)], &[], &selection)
                .and_then(|selected| selected.finish(&[]))
                .map(|(favored, _)| {
                    favored
                        .iter()
                        .map(|record| record.identifier.to_file_name())
                        .collect::<Vec<_>>()
                })
        };

        assert_eq!(
            load("default", Subdir::Linux64).unwrap(),
            ["bar-1.0-0.tar.bz2", "foo-1.0-0.tar.bz2"]
        );
        assert_eq!(
            load("new", Subdir::Linux64).unwrap(),
            ["bar-2.0-0.tar.bz2", "foo-2.0-0.tar.bz2"]
        );

        let error = load("missing", Subdir::Linux64).unwrap_err().to_string();
        assert!(error.contains("'default', 'new', 'old'"), "{error}");
        let error = load("default", Subdir::Win64).unwrap_err().to_string();
        assert!(error.contains("no packages for win-64"), "{error}");
    }

    fn record(name: &str, version: &str, url: &str) -> RepoDataRecord {
        let file_name = format!("{name}-{version}-0.tar.bz2");
        let package_record = rattler_conda_types::PackageRecord::new(
            PackageName::from_str(name).unwrap(),
            rattler_conda_types::VersionWithSource::from_str(version).unwrap(),
            "0".parse().unwrap(),
        );
        RepoDataRecord {
            package_record,
            identifier: file_name.parse().unwrap(),
            url: Url::parse(&format!("{url}/{file_name}")).unwrap(),
            channel: None,
        }
    }

    #[test]
    fn test_finish_dedupes() {
        let foo_1 = record("foo", "1", "https://a.example.com");
        let foo_2 = record("foo", "2", "https://a.example.com");
        let bar_1 = record("bar", "1", "https://a.example.com");
        let selected = SelectedRecords {
            favor: vec![
                Slot::Records(vec![foo_1.clone(), bar_1.clone()]),
                Slot::Records(vec![foo_1.clone(), foo_2.clone()]),
            ],
            pin: vec![],
        };
        let (favored, pinned) = selected.finish(&[]).unwrap();
        assert_eq!(favored, [foo_1.clone(), bar_1.clone()]);
        assert!(pinned.is_empty());

        // A pin wins over a favor of the same package.
        let selected = SelectedRecords {
            favor: vec![Slot::Records(vec![foo_1.clone(), bar_1.clone()])],
            pin: vec![Slot::Records(vec![foo_2.clone()])],
        };
        let (favored, pinned) = selected.finish(&[]).unwrap();
        assert_eq!(favored, [bar_1]);
        assert_eq!(pinned, std::slice::from_ref(&foo_2));

        // Two different pins of the same package cannot both be satisfied.
        let selected = SelectedRecords {
            favor: vec![],
            pin: vec![Slot::Records(vec![foo_1]), Slot::Records(vec![foo_2])],
        };
        let error = selected.finish(&[]).unwrap_err().to_string();
        assert!(error.contains("'foo' is pinned to two different packages"));
    }

    #[test]
    fn test_query_names() {
        let mut foo = record("foo", "1", "https://a.example.com");
        foo.package_record.depends = vec!["bar >=1".to_string(), "__unix".to_string()];
        let selected = SelectedRecords {
            favor: vec![Slot::Records(vec![foo])],
            pin: vec![Slot::Remote {
                url: Url::parse("https://a.example.com/baz-1-0.conda").unwrap(),
                name: PackageName::from_str("baz").unwrap(),
                record: None,
            }],
        };
        let names = selected
            .query_names()
            .into_iter()
            .map(|name| name.as_normalized().to_string())
            .collect::<Vec<_>>();
        assert_eq!(names, ["__unix", "bar", "baz"]);
    }
}
