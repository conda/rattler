//! This module provides the [`SparseRepoData`] which is a struct to enable only
//! sparsely loading records from a `repodata.json` file.

#![allow(clippy::mem_forget)]

mod format_bucket;

#[cfg(feature = "gateway")]
use std::sync::Arc;
use std::{
    borrow::Borrow,
    collections::{HashSet, VecDeque},
    fmt, io,
    marker::PhantomData,
    path::Path,
    str::FromStr,
    sync::LazyLock,
};

use bytes::Bytes;
#[cfg(feature = "gateway")]
pub(crate) use format_bucket::FormatBucketMap;
pub(crate) use format_bucket::{FormatBucket, FormatBucketSet};
use itertools::{Either, EitherOrBoth, Itertools};
use rattler_conda_types::{
    Channel, ChannelInfo, ChannelRelations, MatchSpec, Matches, PackageName, PackageRecord,
    RepoDataRecord, RepodataRevisions, UrlOrPath, WhlPackageRecord, compute_package_url,
    package::{
        ArchiveIdentifier, CondaArchiveType, DistArchiveIdentifier, DistArchiveType,
        WheelArchiveType,
    },
};
use rattler_redaction::Redact;
use serde::{
    Deserialize, Deserializer,
    de::{Error, MapAccess, Visitor},
};
use serde_json::value::RawValue;
use superslice::Ext;
use thiserror::Error;
use url::Url;

/// Shared empty revisions, returned by accessors when none are advertised.
pub(crate) fn empty_repodata_revisions() -> &'static RepodataRevisions {
    static EMPTY: LazyLock<RepodataRevisions> = LazyLock::new(RepodataRevisions::new);
    &EMPTY
}

/// Selects which archive formats of a package are used when the same build
/// (the same `name-version-build`) is available in more than one format.
///
/// Removed packages are dropped before formats are compared: if the `.conda`
/// file of a build is removed, its `.tar.bz2` file counts as having no
/// `.conda` counterpart. Where formats are preferred over each other the
/// order is `.conda` over `.whl` over `.tar.bz2`.
#[derive(
    Default,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    strum::Display,
    strum::VariantNames,
    strum::EnumString,
    strum::IntoStaticStr,
)]
#[strum(serialize_all = "kebab-case")]
#[non_exhaustive]
pub enum PackageFormatSelection {
    /// Only `.tar.bz2` packages are used.
    OnlyTarBz2,

    /// Only `.conda` packages are used.
    OnlyConda,

    /// `.conda` and `.tar.bz2` packages are used. A `.tar.bz2` package is
    /// discarded if the same build is available as `.conda`. Wheels are not
    /// used.
    #[default]
    PreferConda,

    /// `.conda`, `.whl` and `.tar.bz2` packages are used, but only the most
    /// preferred format of each build: `.conda` over `.whl` over `.tar.bz2`.
    PreferCondaWithWhl,

    /// `.conda` and `.tar.bz2` packages are used, both when they represent the
    /// same build. Wheels are not used.
    Both,

    /// Every package is used regardless of its format, without discarding any
    /// format in favor of another.
    All,
}

/// A package that a repodata index lists under its `removed` key. The archive
/// may still be downloadable, but the channel no longer offers it for
/// installation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemovedPackage {
    /// The URL the archive was served from. Derived the same way as
    /// [`RepoDataRecord::url`], so it compares directly against the URL of a
    /// previously fetched record or a lock file entry.
    pub url: Url,

    /// The identifier parsed from the removed file name.
    pub identifier: DistArchiveIdentifier,

    /// The channel the package was removed from, see
    /// [`RepoDataRecord::channel`].
    pub channel: Option<String>,
}

/// A struct to enable loading records from a `repodata.json` file on demand.
/// Since most of the time you don't need all the records from the
/// `repodata.json` this can help provide some significant speedups.
pub struct SparseRepoData {
    /// Data structure that holds an index into the the records stored in a repo
    /// data.
    inner: SparseRepoDataInner,

    /// The channel from which this data was downloaded.
    pub channel: Channel,

    /// The subdirectory from where the repodata is downloaded
    subdir: String,

    /// A function that can be used to patch the package record after it has
    /// been parsed. This is mainly used to add `pip` to `python` if desired
    patch_record_fn: Option<fn(&mut PackageRecord)>,
}

enum SparseRepoDataInner {
    /// The repo data is stored as a memory mapped file
    #[cfg(any(unix, windows))]
    Memmapped(MemmappedSparseRepoDataInner),
    /// The repo data is stored as `Bytes`
    Bytes(BytesSparseRepoDataInner),
}

impl SparseRepoDataInner {
    fn borrow_repo_data(&self) -> &LazyRepoData<'_> {
        match self {
            #[cfg(any(unix, windows))]
            SparseRepoDataInner::Memmapped(inner) => inner.borrow_dependent(),
            SparseRepoDataInner::Bytes(inner) => inner.borrow_dependent(),
        }
    }
}

// A struct that holds a memory map of a `repodata.json` file and also a
// self-referential field which indexes the data in the memory map with a
// sparsely parsed json struct. See [`LazyRepoData`].
#[cfg(any(unix, windows))]
self_cell::self_cell!(
    struct MemmappedSparseRepoDataInner {
        // Memory map of the `repodata.json` file
        owner: memmap2::Mmap,

        // Sparsely parsed json content of the memory map. This data struct holds
        // references into the memory map so we have to use ouroboros to make
        // this legal.
        #[covariant]
        dependent: LazyRepoData,
    }
);

// A struct that holds a reference to the bytes of a `repodata.json` file and
// also a self-referential field which indexes the data in the `bytes` with a
// sparsely parsed json struct. See [`LazyRepoData`].
self_cell::self_cell!(
    struct BytesSparseRepoDataInner {
        // Bytes of the `repodata.json` file
        owner: Bytes,

        // Sparsely parsed json content of the file's bytes. This data struct holds
        // references into the bytes so we have to use ouroboros to make this
        // legal.
        #[covariant]
        dependent: LazyRepoData,
    }
);

impl SparseRepoData {
    /// Construct an instance of self from a file on disk and a [`Channel`].
    ///
    /// The `patch_function` can be used to patch the package record after it
    /// has been parsed (e.g. to add `pip` to `python`).
    ///
    /// On Windows the file is opened with `FILE_SHARE_DELETE` so that another
    /// process/thread can rename or delete the file while it is still mapped.
    #[cfg(any(unix, windows))]
    pub fn from_file(
        channel: Channel,
        subdir: impl Into<String>,
        path: impl AsRef<Path>,
        patch_function: Option<fn(&mut PackageRecord)>,
    ) -> Result<Self, io::Error> {
        #[cfg(windows)]
        let file = {
            use std::os::windows::fs::OpenOptionsExt;
            const SHARE_ALL: u32 = 0x01 | 0x02 | 0x04; // FILE_SHARE_READ | WRITE | DELETE
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(SHARE_ALL)
                .open(path.as_ref())?
        };
        #[cfg(not(windows))]
        let file = std::fs::File::from(fs_err::File::open(path.as_ref())?);

        let memory_map = unsafe { memmap2::Mmap::map(&file) }?;
        Ok(SparseRepoData {
            inner: SparseRepoDataInner::Memmapped(MemmappedSparseRepoDataInner::try_new(
                memory_map,
                |memory_map| serde_json::from_slice(memory_map.as_ref()),
            )?),
            subdir: subdir.into(),
            channel,
            patch_record_fn: patch_function,
        })
    }

    /// Construct an instance of self from a file on disk and a [`Channel`].
    ///
    /// The `patch_function` can be used to patch the package record after it
    /// has been parsed (e.g. to add `pip` to `python`).
    #[cfg(not(any(windows, unix)))]
    pub fn from_file(
        channel: Channel,
        subdir: impl Into<String>,
        path: impl AsRef<Path>,
        patch_function: Option<fn(&mut PackageRecord)>,
    ) -> Result<Self, io::Error> {
        let bytes = fs_err::read(path)?;
        Ok(Self::from_bytes(
            channel,
            subdir,
            bytes.into(),
            patch_function,
        )?)
    }

    /// Construct an instance of self from a bytes and a [`Channel`].
    ///
    /// The `patch_function` can be used to patch the package record after it
    /// has been parsed (e.g. to add `pip` to `python`).
    pub fn from_bytes(
        channel: Channel,
        subdir: impl Into<String>,
        bytes: Bytes,
        patch_function: Option<fn(&mut PackageRecord)>,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            inner: SparseRepoDataInner::Bytes(BytesSparseRepoDataInner::try_new(bytes, |bytes| {
                serde_json::from_slice(bytes)
            })?),
            channel,
            subdir: subdir.into(),
            patch_record_fn: patch_function,
        })
    }

    /// Returns an iterator over the names of all packages in this repodata
    /// file that have at least one record in the given package format
    /// selection. The names are sorted and unique.
    pub fn package_names(
        &self,
        package_format_selection: PackageFormatSelection,
    ) -> impl Iterator<Item = &'_ str> {
        let repo_data = self.inner.borrow_repo_data();
        let buckets = package_format_selection.buckets();
        repo_data.package_names().filter(move |name| {
            repo_data
                .package_entries(name)
                .plan(buckets)
                .next()
                .is_some()
        })
    }

    /// Returns the number of records in this instance for the given package
    /// format selection.
    pub fn record_count(&self, package_format_selection: PackageFormatSelection) -> usize {
        let repo_data = self.inner.borrow_repo_data();
        let buckets = package_format_selection.buckets();
        repo_data
            .package_names()
            .map(|name| repo_data.package_entries(name).plan(buckets).count())
            .sum()
    }

    /// Returns all the records that matches any of the specified match spec.
    pub fn load_matching_records(
        &self,
        spec: impl IntoIterator<Item = impl Borrow<MatchSpec>>,
        package_format_selection: PackageFormatSelection,
    ) -> io::Result<Vec<RepoDataRecord>> {
        let repo_data = self.inner.borrow_repo_data();
        let parser = RecordParser::new(
            &self.channel,
            &self.subdir,
            repo_data.base_url(),
            self.patch_record_fn,
        )?;
        let buckets = package_format_selection.buckets();
        let mut result = Vec::new();
        for (package_name, specs) in &spec.into_iter().chunk_by(|spec| spec.borrow().name.clone()) {
            let grouped_specs = specs.into_iter().collect::<Vec<_>>();
            // TODO: support glob/regex package names
            let package_name = package_name.as_exact().map(PackageName::as_normalized);
            for file in repo_data.plan(package_name, buckets) {
                let record = parser.parse(file.entry)?;
                if grouped_specs
                    .iter()
                    .any(|spec| spec.borrow().matches(&record.package_record))
                {
                    result.push(record);
                }
            }
        }

        Ok(result)
    }

    /// Returns all the records for the specified package name.
    pub fn load_records(
        &self,
        package_name: &PackageName,
        package_format_selection: PackageFormatSelection,
    ) -> io::Result<Vec<RepoDataRecord>> {
        let repo_data = self.inner.borrow_repo_data();
        let parser = RecordParser::new(
            &self.channel,
            &self.subdir,
            repo_data.base_url(),
            self.patch_record_fn,
        )?;
        repo_data
            .plan(
                Some(package_name.as_normalized()),
                package_format_selection.buckets(),
            )
            .map(|file| parser.parse(file.entry))
            .collect()
    }

    /// Parses the records of `package_name` in the given buckets, grouped by
    /// bucket. The records of buckets outside of `buckets` are left empty.
    /// Also returns the packages of that name listed as removed.
    #[cfg(feature = "gateway")]
    pub(crate) fn load_package_buckets(
        &self,
        package_name: &PackageName,
        buckets: FormatBucketSet,
    ) -> io::Result<SparsePackage> {
        let repo_data = self.inner.borrow_repo_data();
        let parser = RecordParser::new(
            &self.channel,
            &self.subdir,
            repo_data.base_url(),
            self.patch_record_fn,
        )?;
        let mut records = FormatBucketMap::<Vec<Arc<RepoDataRecord>>>::default();
        for file in repo_data.plan(Some(package_name.as_normalized()), buckets) {
            records[file.bucket].push(Arc::new(parser.parse(file.entry)?));
        }
        let removed = find_removed_in_slice(&repo_data.removed, Some(package_name))
            .iter()
            .map(|filename| parser.removed_package(filename.filename))
            .collect::<io::Result<_>>()?;
        Ok(SparsePackage { records, removed })
    }

    /// Returns the packages listed under the `removed` key of the repodata for
    /// the specified package name, or for every package when `None` is passed.
    ///
    /// Removed packages are never returned by the `load_*` record functions.
    pub fn load_removed(
        &self,
        package_name: Option<&PackageName>,
    ) -> io::Result<Vec<RemovedPackage>> {
        let repo_data = self.inner.borrow_repo_data();
        let parser = RecordParser::new(
            &self.channel,
            &self.subdir,
            repo_data.base_url(),
            self.patch_record_fn,
        )?;
        find_removed_in_slice(&repo_data.removed, package_name)
            .iter()
            .map(|filename| parser.removed_package(filename.filename))
            .collect()
    }

    /// Returns all the records for the specified package format selection.
    pub fn load_all_records(
        &self,
        package_format_selection: PackageFormatSelection,
    ) -> io::Result<Vec<RepoDataRecord>> {
        let repo_data = self.inner.borrow_repo_data();
        let parser = RecordParser::new(
            &self.channel,
            &self.subdir,
            repo_data.base_url(),
            self.patch_record_fn,
        )?;
        repo_data
            .plan(None, package_format_selection.buckets())
            .map(|file| parser.parse(file.entry))
            .collect()
    }

    /// Given a set of [`SparseRepoData`]s load all the records for the packages
    /// with the specified names and all packages referenced by their regular
    /// or optional dependencies.
    ///
    /// This parses the records for the specified packages as well as all
    /// packages they may depend on, including packages in `extra_depends`.
    /// Only the dependencies of records in the package format selection are
    /// followed.
    pub fn load_records_recursive<'a>(
        repo_data: impl IntoIterator<Item = &'a SparseRepoData>,
        package_names: impl IntoIterator<Item = PackageName>,
        patch_function: Option<fn(&mut PackageRecord)>,
        package_format_selection: PackageFormatSelection,
    ) -> io::Result<Vec<Vec<RepoDataRecord>>> {
        let repo_data: Vec<_> = repo_data.into_iter().collect();
        let parsers = repo_data
            .iter()
            .map(|repo_data| {
                RecordParser::new(
                    &repo_data.channel,
                    &repo_data.subdir,
                    repo_data.inner.borrow_repo_data().base_url(),
                    patch_function,
                )
            })
            .collect::<io::Result<Vec<_>>>()?;
        let buckets = package_format_selection.buckets();

        // Construct the result map
        let mut result: Vec<_> = (0..repo_data.len()).map(|_| Vec::new()).collect();

        // Construct a set of packages that we have seen and have been added to the
        // pending list.
        let mut seen: HashSet<PackageName> = package_names.into_iter().collect();

        // Construct a queue to store packages in that still need to be processed
        let mut pending: VecDeque<_> = seen.iter().cloned().collect();

        // Iterate over the list of packages that still need to be processed.
        while let Some(next_package) = pending.pop_front() {
            for ((repo_data, parser), records) in repo_data.iter().zip(&parsers).zip(&mut result) {
                for file in repo_data
                    .inner
                    .borrow_repo_data()
                    .plan(Some(next_package.as_normalized()), buckets)
                {
                    let record = parser.parse(file.entry)?;

                    // Queue the dependencies of the record that were not seen yet.
                    for dependency in record
                        .package_record
                        .depends
                        .iter()
                        .chain(record.package_record.extra_depends.values().flatten())
                    {
                        let dependency_name = PackageName::from_matchspec_str_unchecked(dependency);
                        if !seen.contains(&dependency_name) {
                            pending.push_back(dependency_name.clone());
                            seen.insert(dependency_name);
                        }
                    }

                    records.push(record);
                }
            }
        }

        Ok(result)
    }

    /// Returns the subdirectory from which this repodata was loaded
    pub fn subdir(&self) -> &str {
        &self.subdir
    }

    /// Returns the repodata revisions advertised by this repodata file.
    pub fn repodata_revisions(&self) -> &RepodataRevisions {
        match &self.inner.borrow_repo_data().info {
            Some(info) => &info.repodata_revisions,
            None => empty_repodata_revisions(),
        }
    }

    /// CEP-42 channel relations from `info.channel_relations`, if any.
    pub fn channel_relations(&self) -> Option<&ChannelRelations> {
        self.inner
            .borrow_repo_data()
            .info
            .as_ref()?
            .channel_relations
            .as_ref()
    }
}

/// A serde compatible struct that only sparsely parses a repodata.json file.
#[derive(Deserialize)]
struct LazyRepoData<'i> {
    /// The channel information contained in the repodata.json file
    info: Option<ChannelInfo>,

    /// The tar.bz2 packages contained in the repodata.json file
    #[serde(borrow, default, deserialize_with = "deserialize_legacy_entries")]
    packages: Vec<(PackageFilename<'i>, &'i RawValue)>,

    /// The conda packages contained in the repodata.json file (under a
    /// different key for backwards compatibility with previous conda
    /// versions)
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_legacy_entries",
        rename = "packages.conda"
    )]
    conda_packages: Vec<(PackageFilename<'i>, &'i RawValue)>,

    /// Packages stored under the `v3` top-level key.
    #[serde(borrow, default)]
    v3: LazyV3Packages<'i>,

    /// File names listed under the `removed` key, sorted by package name.
    #[serde(borrow, default, deserialize_with = "deserialize_sorted_filenames")]
    removed: Vec<PackageFilename<'i>>,
}

/// Lazily parsed `v3` section of repodata containing sub-maps for each archive
/// type.
#[derive(Deserialize, Default)]
struct LazyV3Packages<'i> {
    /// v3 tar.bz2 packages
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_v3_entries",
        rename = "tar.bz2"
    )]
    tar_bz2: Vec<(PackageFilename<'i>, &'i RawValue)>,

    /// v3 conda packages
    #[serde(borrow, default, deserialize_with = "deserialize_v3_entries")]
    conda: Vec<(PackageFilename<'i>, &'i RawValue)>,

    /// v3 whl packages
    #[serde(borrow, default, deserialize_with = "deserialize_v3_entries")]
    whl: Vec<(PackageFilename<'i>, &'i RawValue)>,
}

/// An entry of one of the record maps of the repodata: the key and the raw
/// json of the record.
type RawEntry<'i> = (PackageFilename<'i>, &'i RawValue);

/// The records of one package, grouped by [`FormatBucket`], as returned by
/// [`SparseRepoData::load_package_buckets`].
#[cfg(feature = "gateway")]
pub(crate) struct SparsePackage {
    /// The parsed records per bucket.
    pub(crate) records: FormatBucketMap<Vec<Arc<RepoDataRecord>>>,

    /// The packages of this name that the repodata lists as removed.
    pub(crate) removed: Vec<RemovedPackage>,
}

impl<'i> LazyRepoData<'i> {
    /// The `base_url` advertised in the `info` section, if any.
    fn base_url(&self) -> Option<&str> {
        self.info.as_ref().and_then(|info| info.base_url.as_deref())
    }

    /// Iterates over the names of the packages that have an entry in any of
    /// the record maps, sorted and without duplicates.
    fn package_names(&self) -> impl Iterator<Item = &'i str> + '_ {
        [
            &self.packages,
            &self.conda_packages,
            &self.v3.tar_bz2,
            &self.v3.conda,
            &self.v3.whl,
        ]
        .into_iter()
        .map(|entries| entries.iter().map(|(filename, _)| filename.package))
        .kmerge()
        .dedup()
    }

    /// Returns the entries of the package with the given (normalized) name.
    fn package_entries(&self, package_name: &str) -> PackageEntries<'_, 'i> {
        PackageEntries {
            tar_bz2: entries_of_package(&self.packages, package_name),
            conda: entries_of_package(&self.conda_packages, package_name),
            v3_tar_bz2: entries_of_package(&self.v3.tar_bz2, package_name),
            v3_conda: entries_of_package(&self.v3.conda, package_name),
            v3_whl: entries_of_package(&self.v3.whl, package_name),
            removed: &self.removed[self
                .removed
                .equal_range_by(|filename| filename.package.cmp(package_name))],
        }
    }

    /// Plans the files in `buckets` of the package with the given
    /// (normalized) name, or of every package if no name is given.
    fn plan<'a>(
        &'a self,
        package_name: Option<&str>,
        buckets: FormatBucketSet,
    ) -> impl Iterator<Item = PlannedFile<'i>> + 'a {
        match package_name {
            Some(package_name) => Either::Left(self.package_entries(package_name).plan(buckets)),
            None => Either::Right(
                self.package_names()
                    .flat_map(move |package_name| self.package_entries(package_name).plan(buckets)),
            ),
        }
    }
}

/// The entries of a single package in every record map of the repodata.
/// The record maps are sorted by stem (see [`RecordKind::stem`] and
/// [`deserialize_filename_and_raw_record`]) and `removed` by file name (see
/// [`deserialize_sorted_filenames`]).
#[derive(Clone, Copy)]
struct PackageEntries<'a, 'i> {
    /// Entries of `packages`.
    tar_bz2: &'a [RawEntry<'i>],
    /// Entries of `packages.conda`.
    conda: &'a [RawEntry<'i>],
    /// Entries of `v3.tar.bz2`.
    v3_tar_bz2: &'a [RawEntry<'i>],
    /// Entries of `v3.conda`.
    v3_conda: &'a [RawEntry<'i>],
    /// Entries of `v3.whl`.
    v3_whl: &'a [RawEntry<'i>],
    /// File names listed under `removed`.
    removed: &'a [PackageFilename<'i>],
}

/// A file of a package in the repodata that is not removed.
#[derive(Clone, Copy)]
struct FileEntry<'i> {
    /// The file name without archive extension.
    stem: &'i str,
    archive_type: DistArchiveType,
    filename: PackageFilename<'i>,
    raw_json: &'i RawValue,
    kind: RecordKind,
}

/// A file selected for parsing by [`PackageEntries::plan`].
#[derive(Clone, Copy)]
struct PlannedFile<'i> {
    #[cfg_attr(
        not(feature = "gateway"),
        expect(dead_code, reason = "only the gateway groups records by bucket")
    )]
    bucket: FormatBucket,
    entry: FileEntry<'i>,
}

/// How the key of a record map entry is spelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordKind {
    /// An entry of `packages` or `packages.conda`, keyed by its file name.
    Legacy,
    /// An entry of a `v3` map, keyed by its archive identifier: the file name
    /// without extension.
    V3,
}

impl RecordKind {
    /// Returns the stem of a key of this kind: the archive identifier of the
    /// file, i.e. its file name without archive extension. A `v3` key already
    /// is the identifier and is returned unchanged, even if its build string
    /// happens to end in something that looks like an archive extension.
    fn stem(self, key: &str) -> &str {
        match self {
            RecordKind::Legacy => DistArchiveType::split_str(key).map_or(key, |(stem, _)| stem),
            RecordKind::V3 => key,
        }
    }
}

impl<'a, 'i> PackageEntries<'a, 'i> {
    /// The legacy and `v3` entries that hold files of the given archive type.
    fn sections(self, archive_type: DistArchiveType) -> (&'a [RawEntry<'i>], &'a [RawEntry<'i>]) {
        match archive_type {
            DistArchiveType::Conda(CondaArchiveType::Conda) => (self.conda, self.v3_conda),
            DistArchiveType::Conda(CondaArchiveType::TarBz2) => (self.tar_bz2, self.v3_tar_bz2),
            DistArchiveType::Wheel(WheelArchiveType::Whl) => (&[], self.v3_whl),
        }
    }

    /// Returns true if the file with the given stem and archive type is
    /// listed under `removed`.
    fn is_removed(self, stem: &str, archive_type: DistArchiveType) -> bool {
        let extension = archive_type.extension();
        self.removed
            .binary_search_by(|removed| {
                removed
                    .filename
                    .bytes()
                    .cmp(stem.bytes().chain(extension.bytes()))
            })
            .is_ok()
    }

    /// Returns true if the package has a file with the given stem and archive
    /// type that is not removed.
    fn has_file(self, stem: &str, archive_type: DistArchiveType) -> bool {
        let (legacy, v3) = self.sections(archive_type);
        let is_listed = |entries: &[RawEntry<'i>], kind: RecordKind| {
            entries
                .binary_search_by(|(filename, _)| kind.stem(filename.filename).cmp(stem))
                .is_ok()
        };
        (is_listed(legacy, RecordKind::Legacy) || is_listed(v3, RecordKind::V3))
            && !self.is_removed(stem, archive_type)
    }

    /// Iterates over the files of the given archive type that are not
    /// removed, sorted by stem. Iterates over nothing if no bucket in
    /// `buckets` holds files of this archive type.
    fn files(
        self,
        archive_type: DistArchiveType,
        buckets: FormatBucketSet,
    ) -> impl Iterator<Item = FileEntry<'i>> + 'a {
        let (legacy, v3) = if buckets.contains_archive_type(archive_type) {
            self.sections(archive_type)
        } else {
            (&[][..], &[][..])
        };
        let legacy = legacy.iter().map(move |&(filename, raw_json)| FileEntry {
            stem: RecordKind::Legacy.stem(filename.filename),
            archive_type,
            filename,
            raw_json,
            kind: RecordKind::Legacy,
        });
        let v3 = v3.iter().map(move |&(filename, raw_json)| FileEntry {
            stem: RecordKind::V3.stem(filename.filename),
            archive_type,
            filename,
            raw_json,
            kind: RecordKind::V3,
        });
        legacy
            .merge_join_by(v3, |legacy, v3| legacy.stem.cmp(v3.stem))
            .map(|entry| match entry {
                // A file listed both in a legacy map and in `v3` is described
                // by its `v3` entry.
                EitherOrBoth::Both(_, entry)
                | EitherOrBoth::Left(entry)
                | EitherOrBoth::Right(entry) => entry,
            })
            .filter(move |entry| !self.is_removed(entry.stem, archive_type))
    }

    /// Iterates over the files of this package that fall into one of
    /// `buckets`, sorted by stem and, for files of the same build, in order of
    /// archive type preference.
    fn plan(self, buckets: FormatBucketSet) -> impl Iterator<Item = PlannedFile<'i>> + 'a {
        let conda = self.files(CondaArchiveType::Conda.into(), buckets);
        let whl = self.files(WheelArchiveType::Whl.into(), buckets);
        let tar_bz2 = self.files(CondaArchiveType::TarBz2.into(), buckets);
        conda
            .merge_by(whl, |left, right| left.stem <= right.stem)
            .merge_by(tar_bz2, |left, right| left.stem <= right.stem)
            .filter_map(move |entry| {
                let bucket = FormatBucket::classify(entry.archive_type, |twin| {
                    self.has_file(entry.stem, twin)
                });
                buckets
                    .contains(bucket)
                    .then_some(PlannedFile { bucket, entry })
            })
    }
}

/// Returns the entries of a record map that belong to the package with the
/// given (normalized) name.
fn entries_of_package<'a, 'i>(
    entries: &'a [RawEntry<'i>],
    package_name: &str,
) -> &'a [RawEntry<'i>] {
    &entries[entries.equal_range_by(|(filename, _)| filename.package.cmp(package_name))]
}

/// Returns the removed file names that belong to the given package name, or
/// all of them when no name is given.
fn find_removed_in_slice<'a, 'i>(
    slice: &'a [PackageFilename<'i>],
    package_name: Option<&PackageName>,
) -> &'a [PackageFilename<'i>] {
    let range = match package_name {
        None => 0..slice.len(),
        Some(package_name) => {
            slice.equal_range_by(|filename| filename.package.cmp(package_name.as_normalized()))
        }
    };
    &slice[range]
}

/// Converts raw repodata entries of one subdirectory of a channel into
/// records.
struct RecordParser<'a> {
    /// The URL of the subdirectory, used to resolve relative package URLs.
    subdir_url: Url,
    /// The `base_url` advertised by the repodata, if any.
    base_url: Option<&'a str>,
    /// The redacted channel URL stored in every record.
    channel_name: String,
    subdir: &'a str,
    patch_function: Option<fn(&mut PackageRecord)>,
}

impl<'a> RecordParser<'a> {
    fn new(
        channel: &Channel,
        subdir: &'a str,
        base_url: Option<&'a str>,
        patch_function: Option<fn(&mut PackageRecord)>,
    ) -> io::Result<Self> {
        let subdir_url = channel
            .base_url
            .url()
            .join(&format!("{subdir}/"))
            .map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("failed to determine the url of subdir '{subdir}': {err}"),
                )
            })?;
        Ok(Self {
            subdir_url,
            base_url,
            channel_name: channel.base_url.url().clone().redact().to_string(),
            subdir,
            patch_function,
        })
    }

    /// Parses the record of a file.
    fn parse(&self, entry: FileEntry<'_>) -> io::Result<RepoDataRecord> {
        let mut record = match (entry.kind, entry.archive_type) {
            // Legacy keys are complete file names, which include the archive
            // type.
            (RecordKind::Legacy, DistArchiveType::Conda(_) | DistArchiveType::Wheel(_)) => {
                let package_record = self.parse_package_record(entry.raw_json)?;
                let identifier: DistArchiveIdentifier = parse_identifier(entry.filename)?;
                RepoDataRecord {
                    url: compute_package_url(
                        &self.subdir_url,
                        self.base_url,
                        &identifier.to_string(),
                    ),
                    channel: Some(self.channel_name.clone()),
                    package_record,
                    identifier,
                }
            }
            (RecordKind::V3, archive_type @ DistArchiveType::Conda(_)) => {
                let package_record = self.parse_package_record(entry.raw_json)?;
                let identifier =
                    DistArchiveIdentifier::new(parse_identifier(entry.filename)?, archive_type);
                RepoDataRecord {
                    url: compute_package_url(
                        &self.subdir_url,
                        self.base_url,
                        &identifier.to_file_name(),
                    ),
                    channel: Some(self.channel_name.clone()),
                    package_record,
                    identifier,
                }
            }
            (RecordKind::V3, DistArchiveType::Wheel(WheelArchiveType::Whl)) => {
                let WhlPackageRecord {
                    url,
                    mut package_record,
                } = serde_json::from_str(entry.raw_json.get())?;
                self.default_subdir(&mut package_record);
                let identifier: ArchiveIdentifier = parse_identifier(entry.filename)?;
                RepoDataRecord {
                    url: match url {
                        UrlOrPath::Path(path) => {
                            compute_package_url(&self.subdir_url, self.base_url, &path)
                        }
                        UrlOrPath::Url(url) => url,
                    },
                    channel: Some(self.channel_name.clone()),
                    package_record,
                    identifier: DistArchiveIdentifier::new(identifier, WheelArchiveType::Whl),
                }
            }
        };

        if let Some(patch_function) = self.patch_function {
            patch_function(&mut record.package_record);
        }

        Ok(record)
    }

    /// Parses a conda package record, filling in the subdir if it is empty.
    fn parse_package_record(&self, raw_json: &RawValue) -> io::Result<PackageRecord> {
        let mut package_record: PackageRecord = serde_json::from_str(raw_json.get())?;
        self.default_subdir(&mut package_record);
        Ok(package_record)
    }

    /// Sets the subdir of the record to the subdir of the repodata if it is
    /// empty.
    fn default_subdir(&self, package_record: &mut PackageRecord) {
        if package_record.subdir.is_empty() {
            self.subdir.clone_into(&mut package_record.subdir);
        }
    }

    /// Describes a file name listed under `removed`.
    fn removed_package(&self, filename: &str) -> io::Result<RemovedPackage> {
        let identifier: DistArchiveIdentifier = filename.parse().map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid archive identifier '{filename}': {err}"),
            )
        })?;
        Ok(RemovedPackage {
            url: compute_package_url(&self.subdir_url, self.base_url, filename),
            identifier,
            channel: Some(self.channel_name.clone()),
        })
    }
}

/// Parses an archive identifier from the key of a record map entry.
fn parse_identifier<T: FromStr<Err: fmt::Display>>(filename: PackageFilename<'_>) -> io::Result<T> {
    filename.filename.parse().map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid archive identifier '{}': {err}", filename.filename),
        )
    })
}

/// A helper function that immediately loads the records for the given packages
/// (and their dependencies). Records for the specified packages are loaded from
/// the repodata files. The `patch_record_fn` is applied to each record after it
/// has been parsed and can mutate the record after it has been loaded.
#[cfg(any(unix, windows))]
pub async fn load_repo_data_recursively(
    repo_data_paths: impl IntoIterator<Item = (Channel, impl Into<String>, impl AsRef<Path>)>,
    package_names: impl IntoIterator<Item = PackageName>,
    patch_function: Option<fn(&mut PackageRecord)>,
    variant_consolidation: PackageFormatSelection,
) -> Result<Vec<Vec<RepoDataRecord>>, io::Error> {
    use futures::{StreamExt, TryFutureExt, TryStreamExt};

    // Open the different files and memory map them to get access to their bytes. Do
    // this in parallel.
    let lazy_repo_data = futures::stream::iter(repo_data_paths)
        .map(|(channel, subdir, path)| {
            let path = path.as_ref().to_path_buf();
            let subdir = subdir.into();
            tokio::task::spawn_blocking(move || {
                SparseRepoData::from_file(channel, subdir, path, patch_function)
            })
            .unwrap_or_else(|r| match r.try_into_panic() {
                Ok(panic) => std::panic::resume_unwind(panic),
                Err(err) => Err(io::Error::other(err.to_string())),
            })
        })
        .buffered(50)
        .try_collect::<Vec<_>>()
        .await?;

    SparseRepoData::load_records_recursive(
        &lazy_repo_data,
        package_names,
        patch_function,
        variant_consolidation,
    )
}

/// Deserializes a record map whose keys are spelled as described by `kind`,
/// sorted by package name and then by stem.
fn deserialize_filename_and_raw_record<'d, D: Deserializer<'d>>(
    deserializer: D,
    kind: RecordKind,
) -> Result<Vec<(PackageFilename<'d>, &'d RawValue)>, D::Error> {
    #[allow(clippy::type_complexity)]
    struct MapVisitor<I, K, V>(PhantomData<fn() -> (I, K, V)>);

    impl<'de, I, K, V> Visitor<'de> for MapVisitor<I, K, V>
    where
        I: FromIterator<(K, V)>,
        K: Deserialize<'de>,
        V: Deserialize<'de>,
    {
        type Value = I;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a map")
        }

        fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let iter = MapIter(map, PhantomData);
            iter.collect()
        }
    }

    struct MapIter<'de, A, K, V>(A, PhantomData<(&'de (), A, K, V)>);

    impl<'de, A, K, V> Iterator for MapIter<'de, A, K, V>
    where
        A: MapAccess<'de>,
        K: Deserialize<'de>,
        V: Deserialize<'de>,
    {
        type Item = Result<(K, V), A::Error>;

        fn next(&mut self) -> Option<Self::Item> {
            match self.0.next_entry() {
                Ok(Some(x)) => Some(Ok(x)),
                Ok(None) => None,
                Err(err) => Some(Err(err)),
            }
        }
    }

    let mut entries: Vec<(PackageFilename<'d>, &'d RawValue)> =
        deserializer.deserialize_map(MapVisitor(PhantomData))?;

    // Although in general the filenames are sorted in repodata.json this doesn't
    // necessarily mean that the records are also sorted by package name.
    //
    // To illustrate, the following filenames are properly sorted by filename but
    // they are NOT properly sorted by package name.
    // - clang-format-12.0.1-default_he082bbe_4.tar.bz2 (package name: clang-format)
    // - clang-format-13-13.0.0-default_he082bbe_0.tar.bz2 (package name:
    //   clang-format-13)
    // - clang-format-13.0.0-default_he082bbe_0.tar.bz2 (package name: clang-format)
    //
    // Because most use-cases involve finding filenames by package name we reorder
    // the entries here by package name. This enables use the binary search for
    // the packages we need.
    //
    // Since (in most cases) the repodata is already ordered by filename which does
    // closely resemble ordering by package name this sort operation will most
    // likely be very fast.
    //
    // Within a package the entries are ordered by their stem (see
    // [`RecordKind::stem`]). Files of the same build in different maps (e.g.
    // `packages` and `v3.tar.bz2`, or `packages.conda` and `packages`) are
    // matched up by stem, which requires every map to be sorted the same way.
    entries.sort_unstable_by(|(a, _), (b, _)| {
        a.package
            .cmp(b.package)
            .then_with(|| kind.stem(a.filename).cmp(kind.stem(b.filename)))
    });

    Ok(entries)
}

/// Deserializes a legacy record map (`packages` or `packages.conda`), see
/// [`deserialize_filename_and_raw_record`].
fn deserialize_legacy_entries<'d, D: Deserializer<'d>>(
    deserializer: D,
) -> Result<Vec<(PackageFilename<'d>, &'d RawValue)>, D::Error> {
    deserialize_filename_and_raw_record(deserializer, RecordKind::Legacy)
}

/// Deserializes a `v3` record map, see
/// [`deserialize_filename_and_raw_record`].
fn deserialize_v3_entries<'d, D: Deserializer<'d>>(
    deserializer: D,
) -> Result<Vec<(PackageFilename<'d>, &'d RawValue)>, D::Error> {
    deserialize_filename_and_raw_record(deserializer, RecordKind::V3)
}

/// Deserializes a list of file names and sorts it by package name so entries
/// for a single package can be found with a binary search.
fn deserialize_sorted_filenames<'d, D: Deserializer<'d>>(
    deserializer: D,
) -> Result<Vec<PackageFilename<'d>>, D::Error> {
    let mut entries: Vec<PackageFilename<'d>> = Vec::deserialize(deserializer)?;
    entries.sort_unstable_by(|a, b| {
        a.package
            .cmp(b.package)
            .then_with(|| a.filename.cmp(b.filename))
    });
    Ok(entries)
}

/// A struct that holds both a filename and the part of the filename that is just
/// the package name.
#[derive(Copy, Clone)]
struct PackageFilename<'i> {
    package: &'i str,
    filename: &'i str,
}

impl<'de> Deserialize<'de> for PackageFilename<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        <&str>::deserialize(deserializer)?
            .try_into()
            .map_err(D::Error::custom)
    }
}

/// Error when parsing a package filename
#[derive(Error, Debug)]
pub enum PackageFilenameError {
    /// The package filename must contain at least two `-`
    #[error("package filename ({0}) must contain at least two `-`")]
    NotEnoughDashes(String),
}

impl<'de> TryFrom<&'de str> for PackageFilename<'de> {
    type Error = PackageFilenameError;

    fn try_from(s: &'de str) -> Result<Self, Self::Error> {
        let package = s
            .rsplitn(3, '-')
            .nth(2)
            .ok_or(PackageFilenameError::NotEnoughDashes(s.to_string()))?;
        Ok(PackageFilename {
            package,
            filename: s,
        })
    }
}

#[cfg(test)]
mod test {
    use std::{collections::HashSet, path::PathBuf};

    use bytes::Bytes;
    use fs_err as fs;
    use itertools::Itertools;
    use rattler_conda_types::{
        Channel, ChannelConfig, MatchSpec, PackageName, ParseStrictness, RepoData, RepoDataRecord,
        RepodataRevision,
    };
    use rstest::rstest;
    use url::Url;

    use super::{
        PackageFilename, PackageFormatSelection, SparseRepoData, load_repo_data_recursively,
    };

    fn test_dir() -> PathBuf {
        tools::test_data_dir()
    }

    async fn default_repo_data() -> Vec<(Channel, &'static str, PathBuf)> {
        tokio::try_join!(
            tools::fetch_test_conda_forge_repodata_async("linux-64"),
            tools::fetch_test_conda_forge_repodata_async("noarch")
        )
        .unwrap();

        let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
        vec![
            (
                Channel::from_str("conda-forge", &channel_config).unwrap(),
                "noarch",
                test_dir().join("channels/conda-forge/noarch/repodata.json"),
            ),
            (
                Channel::from_str("conda-forge", &channel_config).unwrap(),
                "linux-64",
                test_dir().join("channels/conda-forge/linux-64/repodata.json"),
            ),
        ]
    }

    fn dummy_repo_data() -> (Channel, &'static str, PathBuf) {
        let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
        (
            Channel::from_str("dummy", &channel_config).unwrap(),
            "linux-64",
            test_dir().join("channels/dummy/linux-64/repodata.json"),
        )
    }

    fn wheel_repo_data() -> (Channel, &'static str, PathBuf) {
        let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
        (
            Channel::from_str("with-wheels", &channel_config).unwrap(),
            "noarch",
            test_dir().join("channels/with-wheels/noarch/repodata.json"),
        )
    }

    async fn default_repo_data_bytes() -> Vec<(Channel, &'static str, Bytes)> {
        default_repo_data()
            .await
            .into_iter()
            .map(|(channel, subdir, path)| {
                let bytes = fs::read(path).unwrap();
                (channel, subdir, bytes.into())
            })
            .collect()
    }

    fn load_sparse_from_bytes(
        repo_data: &[(Channel, &'static str, Bytes)],
        package_names: impl IntoIterator<Item = impl AsRef<str>>,
        variant_consolidation: PackageFormatSelection,
    ) -> Vec<Vec<RepoDataRecord>> {
        let sparse: Vec<_> = repo_data
            .iter()
            .map(|(channel, subdir, bytes)| {
                SparseRepoData::from_bytes(channel.clone(), *subdir, bytes.clone(), None).unwrap()
            })
            .collect();

        let package_names = package_names
            .into_iter()
            .map(|name| PackageName::try_from(name.as_ref()).unwrap());
        SparseRepoData::load_records_recursive(&sparse, package_names, None, variant_consolidation)
            .unwrap()
    }

    async fn load_sparse(
        package_names: impl IntoIterator<Item = impl AsRef<str>>,
        variant_consolidation: PackageFormatSelection,
    ) -> Vec<Vec<RepoDataRecord>> {
        tokio::try_join!(
            tools::fetch_test_conda_forge_repodata_async("noarch"),
            tools::fetch_test_conda_forge_repodata_async("linux-64")
        )
        .unwrap();

        //"linux-sha=20021d1dff9941ccf189f27404e296c54bc37fc4600c7027b366c03fc0bfa89e"
        //"noarch-sha=05e0c4ce7be29f36949c33cce782f21aecfbdd41f9e3423839670fb38fc5d691"

        load_repo_data_recursively(
            default_repo_data().await,
            package_names
                .into_iter()
                .map(|name| PackageName::try_from(name.as_ref()).unwrap()),
            None,
            variant_consolidation,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn test_empty_sparse_load() {
        let sparse_empty_data =
            load_sparse(Vec::<String>::new(), PackageFormatSelection::default()).await;
        assert_eq!(sparse_empty_data, vec![vec![], vec![]]);
    }

    #[tokio::test]
    async fn test_sparse_single() {
        let sparse_empty_data =
            load_sparse(["_libgcc_mutex"], PackageFormatSelection::default()).await;
        let total_records = sparse_empty_data
            .iter()
            .map(std::vec::Vec::len)
            .sum::<usize>();

        assert_eq!(total_records, 3);
    }

    #[tokio::test]
    async fn test_parse_duplicate() {
        let sparse_empty_data = load_sparse(
            ["_libgcc_mutex", "_libgcc_mutex"],
            PackageFormatSelection::default(),
        )
        .await;
        let total_records = sparse_empty_data
            .iter()
            .map(std::vec::Vec::len)
            .sum::<usize>();

        // Number of records should still be 3. The duplicate package name should be
        // ignored.
        assert_eq!(total_records, 3);
    }

    #[tokio::test]
    async fn test_sparse_jupyterlab_detectron2() {
        let sparse_empty_data = load_sparse(
            ["jupyterlab", "detectron2"],
            PackageFormatSelection::default(),
        )
        .await;

        let total_records = sparse_empty_data
            .iter()
            .map(std::vec::Vec::len)
            .sum::<usize>();

        assert_eq!(total_records, 21732);
    }

    #[tokio::test]
    async fn test_sparse_rubin_env() {
        let sparse_empty_data = load_sparse(["rubin-env"], PackageFormatSelection::default()).await;

        let total_records = sparse_empty_data
            .iter()
            .map(std::vec::Vec::len)
            .sum::<usize>();

        assert_eq!(total_records, 45060);
    }

    #[tokio::test]
    async fn test_sparse_numpy_dev() {
        let package_names = vec![
            "python",
            "cython",
            "compilers",
            "openblas",
            "nomkl",
            "pytest",
            "pytest-cov",
            "pytest-xdist",
            "hypothesis",
            "mypy",
            "typing_extensions",
            "sphinx",
            "numpydoc",
            "ipython",
            "scipy",
            "pandas",
            "matplotlib",
            "pydata-sphinx-theme",
            "pycodestyle",
            "gitpython",
            "cffi",
            "pytz",
        ];

        // Mem-mapped
        let sparse_empty_data =
            load_sparse(package_names.clone(), PackageFormatSelection::default()).await;

        let total_records = sparse_empty_data.iter().map(Vec::len).sum::<usize>();

        assert_eq!(total_records, 16065);

        // Bytes
        let repo_data = default_repo_data_bytes().await;
        let sparse_empty_data =
            load_sparse_from_bytes(&repo_data, package_names, PackageFormatSelection::default());

        let total_records = sparse_empty_data.iter().map(Vec::len).sum::<usize>();

        assert_eq!(total_records, 16065);
    }

    #[tokio::test]
    async fn load_complete_records() {
        tokio::try_join!(
            tools::fetch_test_conda_forge_repodata_async("noarch"),
            tools::fetch_test_conda_forge_repodata_async("linux-64")
        )
        .unwrap();

        let mut records = Vec::new();
        for path in [
            test_dir().join("channels/conda-forge/noarch/repodata.json"),
            test_dir().join("channels/conda-forge/linux-64/repodata.json"),
        ] {
            let str = fs::read_to_string(&path).unwrap();
            let repo_data: RepoData = serde_json::from_str(&str).unwrap();
            records.push(repo_data);
        }

        let total_records = records
            .iter()
            .map(|repo| repo.conda_packages.len() + repo.packages.len())
            .sum::<usize>();

        assert_eq!(total_records, 367596);
    }

    #[rstest]
    #[case("clang-format-13.0.1-root_62800_h69bbbaa_1.conda", "clang-format")]
    #[case("clang-format-13-13.0.1-default_he082bbe_0.tar.bz2", "clang-format-13")]
    fn test_deserialize_package_name(#[case] filename: &str, #[case] result: &str) {
        assert_eq!(PackageFilename::try_from(filename).unwrap().package, result);
    }

    #[test]
    fn test_deserialize_empty_json() {
        let json = r#"{}"#;
        let repo_data: RepoData = serde_json::from_str(json).unwrap();
        let sparse_repodata = SparseRepoData::from_bytes(
            Channel::from_str(
                "conda-forge",
                &ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap()),
            )
            .unwrap(),
            "noarch",
            Bytes::from(json),
            None,
        )
        .unwrap();

        assert_eq!(repo_data.packages.len(), 0);
        assert_eq!(
            sparse_repodata
                .package_names(PackageFormatSelection::default())
                .try_len()
                .unwrap(),
            0
        );
    }

    /// Packages listed under `removed` are hidden from the record loaders and
    /// reported by `load_removed` instead. Keys in the `v3` maps have no
    /// extension, so removal must match on the full file name.
    #[test]
    fn test_removed_packages() {
        let json = r#"{
            "info": {"subdir": "noarch"},
            "packages": {
                "foo-1.0-0.tar.bz2": {
                    "name": "foo", "version": "1.0", "build": "0", "build_number": 0,
                    "subdir": "noarch"
                }
            },
            "packages.conda": {
                "foo-2.0-0.conda": {
                    "name": "foo", "version": "2.0", "build": "0", "build_number": 0,
                    "subdir": "noarch"
                }
            },
            "v3": {
                "conda": {
                    "foo-3.0-0": {
                        "name": "foo", "version": "3.0", "build": "0", "build_number": 0,
                        "subdir": "noarch"
                    }
                }
            },
            "removed": ["foo-3.0-0.conda", "foo-2.0-0.conda", "bar-1.0-0.tar.bz2"]
        }"#;
        let channel = Channel::from_url(Url::parse("https://example.com/channel/").unwrap());
        let sparse =
            SparseRepoData::from_bytes(channel, "noarch", Bytes::from(json), None).unwrap();
        let foo = PackageName::try_from("foo").unwrap();

        let records = sparse
            .load_records(&foo, PackageFormatSelection::Both)
            .unwrap()
            .into_iter()
            .map(|record| record.identifier.to_file_name())
            .collect::<Vec<_>>();
        insta::assert_snapshot!(records.join("\n"), @"foo-1.0-0.tar.bz2");

        let describe = |removed: Vec<super::RemovedPackage>| {
            removed
                .into_iter()
                .map(|removed| {
                    format!(
                        "{} {} {}",
                        removed.url,
                        removed.identifier,
                        removed.channel.as_deref().unwrap_or("-")
                    )
                })
                .join("\n")
        };
        insta::assert_snapshot!(describe(sparse.load_removed(Some(&foo)).unwrap()), @r"
        https://example.com/channel/noarch/foo-2.0-0.conda foo-2.0-0.conda https://example.com/channel/
        https://example.com/channel/noarch/foo-3.0-0.conda foo-3.0-0.conda https://example.com/channel/
        ");
        insta::assert_snapshot!(describe(sparse.load_removed(None).unwrap()), @r"
        https://example.com/channel/noarch/bar-1.0-0.tar.bz2 bar-1.0-0.tar.bz2 https://example.com/channel/
        https://example.com/channel/noarch/foo-2.0-0.conda foo-2.0-0.conda https://example.com/channel/
        https://example.com/channel/noarch/foo-3.0-0.conda foo-3.0-0.conda https://example.com/channel/
        ");
    }

    #[rstest]
    #[case::both(PackageFormatSelection::Both)]
    #[case::prefer_conda(PackageFormatSelection::PreferConda)]
    #[case::prefer_conda_with_whl(PackageFormatSelection::PreferCondaWithWhl)]
    #[case::only_conda(PackageFormatSelection::OnlyConda)]
    fn newer_v3_records_replace_legacy_records(#[case] variant: PackageFormatSelection) {
        let json = r#"{
            "packages.conda": {
                "test-1.0-0.conda": {
                    "name": "test", "version": "1.0", "build": "legacy", "build_number": 0,
                    "subdir": "noarch"
                }
            },
            "v3": {
                "conda": {
                    "test-1.0-0": {
                        "name": "test", "version": "1.0", "build": "v3", "build_number": 0,
                        "subdir": "noarch"
                    }
                }
            }
        }"#;
        let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
        let channel = Channel::from_str("dummy", &channel_config).unwrap();
        let sparse =
            SparseRepoData::from_bytes(channel, "noarch", Bytes::from(json), None).unwrap();

        let records = sparse
            .load_records(&PackageName::try_from("test").unwrap(), variant)
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].package_record.build, "v3");
        assert_eq!(sparse.record_count(variant), 1);
    }

    #[rstest]
    #[case::both(PackageFormatSelection::Both)]
    #[case::prefer_conda(PackageFormatSelection::PreferConda)]
    #[case::prefer_conda_with_whl(PackageFormatSelection::PreferCondaWithWhl)]
    #[case::only_tar_bz2(PackageFormatSelection::OnlyTarBz2)]
    fn newer_v3_tar_bz2_records_replace_legacy_records(#[case] variant: PackageFormatSelection) {
        let json = r#"{
            "packages": {
                "test-1.0-0.tar.bz2": {
                    "name": "test", "version": "1.0", "build": "legacy", "build_number": 0,
                    "subdir": "noarch"
                }
            },
            "v3": {
                "tar.bz2": {
                    "test-1.0-0": {
                        "name": "test", "version": "1.0", "build": "v3", "build_number": 0,
                        "subdir": "noarch"
                    }
                }
            }
        }"#;
        let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
        let channel = Channel::from_str("dummy", &channel_config).unwrap();
        let sparse =
            SparseRepoData::from_bytes(channel, "noarch", Bytes::from(json), None).unwrap();

        let records = sparse
            .load_records(&PackageName::try_from("test").unwrap(), variant)
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].package_record.build, "v3");
        assert_eq!(sparse.record_count(variant), 1);
    }

    #[rstest]
    #[case::both(PackageFormatSelection::Both)]
    #[case::prefer_conda(PackageFormatSelection::PreferConda)]
    #[case::prefer_conda_with_whl(PackageFormatSelection::PreferCondaWithWhl)]
    #[case::only_conda(PackageFormatSelection::OnlyConda)]
    fn legacy_records_are_unchanged_without_v3(#[case] variant: PackageFormatSelection) {
        let json = r#"{
            "packages.conda": {
                "test-1.0-0.conda": {
                    "name": "test", "version": "1.0", "build": "legacy", "build_number": 0,
                    "subdir": "noarch"
                }
            }
        }"#;
        let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
        let channel = Channel::from_str("dummy", &channel_config).unwrap();
        let sparse =
            SparseRepoData::from_bytes(channel, "noarch", Bytes::from(json), None).unwrap();

        let records = sparse
            .load_records(&PackageName::try_from("test").unwrap(), variant)
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].package_record.build, "legacy");
        assert_eq!(sparse.record_count(variant), 1);
    }

    #[rstest]
    #[case::both(PackageFormatSelection::Both)]
    #[case::prefer_conda(PackageFormatSelection::PreferConda)]
    #[case::prefer_conda_with_whl(PackageFormatSelection::PreferCondaWithWhl)]
    #[case::only_tar_bz2(PackageFormatSelection::OnlyTarBz2)]
    #[case::only_conda(PackageFormatSelection::OnlyConda)]
    #[case::all(PackageFormatSelection::All)]
    fn dedup_packages(#[case] variant: PackageFormatSelection) {
        let (channel, platform, path) = dummy_repo_data();
        let sparse = SparseRepoData::from_file(channel, platform, path, None).unwrap();
        let names = sparse.package_names(variant).collect_vec();
        let deduped_names = names.iter().copied().collect::<HashSet<_>>();
        assert_eq!(names.len(), deduped_names.len());
    }

    #[rstest]
    #[case::both(PackageFormatSelection::Both)]
    #[case::prefer_conda(PackageFormatSelection::PreferConda)]
    #[case::prefer_conda_with_whl(PackageFormatSelection::PreferCondaWithWhl)]
    #[case::only_tar_bz2(PackageFormatSelection::OnlyTarBz2)]
    #[case::only_conda(PackageFormatSelection::OnlyConda)]
    #[case::all(PackageFormatSelection::All)]
    fn test_package_format_selection(#[case] variant: PackageFormatSelection) {
        let (channel, platform, path) = dummy_repo_data();
        let sparse = SparseRepoData::from_file(channel, platform, path, None).unwrap();
        let records = sparse
            .load_records(&PackageName::try_from("bors").unwrap(), variant)
            .unwrap()
            .into_iter()
            .map(|record| record.identifier.to_file_name())
            .collect::<Vec<_>>();

        insta::with_settings!({snapshot_suffix => variant.to_string()}, {
            insta::assert_snapshot!(records.join("\n"));
        });
    }

    #[rstest]
    #[case::both(PackageFormatSelection::Both, 29)]
    #[case::prefer_conda(PackageFormatSelection::PreferConda, 25)]
    #[case::prefer_conda_with_whl(PackageFormatSelection::PreferCondaWithWhl, 25)]
    #[case::only_tar_bz2(PackageFormatSelection::OnlyTarBz2, 24)]
    #[case::only_conda(PackageFormatSelection::OnlyConda, 5)]
    #[case::all(PackageFormatSelection::All, 29)]
    fn test_record_count(#[case] variant: PackageFormatSelection, #[case] expected_count: usize) {
        let (channel, platform, path) = dummy_repo_data();
        let sparse = SparseRepoData::from_file(channel, platform, path, None).unwrap();
        let count = sparse.record_count(variant);
        assert_eq!(count, expected_count);
    }

    #[rstest]
    #[case::both(PackageFormatSelection::Both, 6)]
    #[case::prefer_conda(PackageFormatSelection::PreferConda, 4)]
    #[case::prefer_conda_with_whl(PackageFormatSelection::PreferCondaWithWhl, 45)]
    #[case::only_tar_bz2(PackageFormatSelection::OnlyTarBz2, 3)]
    #[case::only_conda(PackageFormatSelection::OnlyConda, 3)]
    #[case::all(PackageFormatSelection::All, 51)]
    fn test_record_count_with_wheels(
        #[case] variant: PackageFormatSelection,
        #[case] expected_count: usize,
    ) {
        let (channel, platform, path) = wheel_repo_data();
        let sparse = SparseRepoData::from_file(channel, platform, path, None).unwrap();
        let count = sparse.record_count(variant);
        assert_eq!(count, expected_count);
    }

    fn format_selection_repo_data() -> SparseRepoData {
        let channel_config = ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap());
        SparseRepoData::from_file(
            Channel::from_str("format-selection", &channel_config).unwrap(),
            "noarch",
            test_dir().join("channels/format-selection/noarch/repodata.json"),
            None,
        )
        .unwrap()
    }

    /// Every selection-taking function agrees with `load_all_records` on
    /// which records a selection contains.
    #[rstest]
    fn selection_functions_agree(
        #[values(
            PackageFormatSelection::OnlyTarBz2,
            PackageFormatSelection::OnlyConda,
            PackageFormatSelection::PreferConda,
            PackageFormatSelection::PreferCondaWithWhl,
            PackageFormatSelection::Both,
            PackageFormatSelection::All
        )]
        selection: PackageFormatSelection,
    ) {
        let sparse = format_selection_repo_data();
        let file_names = |records: Vec<RepoDataRecord>| {
            records
                .into_iter()
                .map(|record| record.identifier.to_file_name())
                .sorted()
                .collect_vec()
        };
        let all_records = file_names(sparse.load_all_records(selection).unwrap());

        assert_eq!(sparse.record_count(selection), all_records.len());

        let names = sparse.package_names(selection).collect_vec();
        let per_name = file_names(
            names
                .iter()
                .flat_map(|name| {
                    sparse
                        .load_records(&PackageName::try_from(*name).unwrap(), selection)
                        .unwrap()
                })
                .collect(),
        );
        assert_eq!(per_name, all_records);

        let matching = file_names(
            sparse
                .load_matching_records(
                    names
                        .iter()
                        .map(|name| MatchSpec::from_str(name, ParseStrictness::Strict).unwrap()),
                    selection,
                )
                .unwrap(),
        );
        assert_eq!(matching, all_records);

        let recursive = file_names(
            SparseRepoData::load_records_recursive(
                [&sparse],
                names
                    .iter()
                    .map(|name| PackageName::try_from(*name).unwrap()),
                None,
                selection,
            )
            .unwrap()
            .concat(),
        );
        assert_eq!(recursive, all_records);
    }

    #[test]
    fn test_repodata_revisions_from_file() {
        // The channel advertises a `v3` revision in the CEP `vN`-keyed
        // dictionary form; make sure we parse it back into the map.
        let (channel, platform, path) = wheel_repo_data();
        let sparse = SparseRepoData::from_file(channel, platform, path, None).unwrap();
        let revisions = sparse.repodata_revisions();
        assert_eq!(revisions.len(), 1);
        let metadata = &revisions[&RepodataRevision::V3];
        assert_eq!(metadata.n_packages, Some(2));
        assert_eq!(
            metadata.oldest.map(|ts| ts.timestamp_millis()),
            Some(1768249989851)
        );
        assert_eq!(
            metadata.newest.map(|ts| ts.timestamp_millis()),
            Some(1773851561010)
        );
    }

    #[test]
    fn test_query() {
        let (channel, platform, path) = dummy_repo_data();
        let sparse = SparseRepoData::from_file(channel, platform, path, None).unwrap();
        let records = sparse
            .load_matching_records(
                vec![
                    MatchSpec::from_str("bors 1.*", ParseStrictness::Lenient).unwrap(),
                    MatchSpec::from_str("issue_717", ParseStrictness::Lenient).unwrap(),
                ],
                PackageFormatSelection::default(),
            )
            .unwrap()
            .into_iter()
            .map(|record| record.identifier.to_file_name())
            .collect::<Vec<_>>();

        insta::assert_snapshot!(records.join("\n"), @r###"
        bors-1.0-bla_1.tar.bz2
        bors-1.1-bla_1.conda
        bors-1.2.1-bla_1.tar.bz2
        issue_717-2.1-bla_1.conda
        "###);
    }

    #[test]
    fn test_nameless_query() {
        let (channel, platform, path) = dummy_repo_data();
        let sparse = SparseRepoData::from_file(channel, platform, path, None).unwrap();
        let records = sparse
            .load_matching_records(
                vec![MatchSpec::from_str("cuda-version 12.5", ParseStrictness::Lenient).unwrap()],
                PackageFormatSelection::default(),
            )
            .unwrap()
            .into_iter()
            .map(|record| record.identifier.to_file_name())
            .collect::<Vec<_>>();

        insta::assert_snapshot!(records.join("\n"), @"cuda-version-12.5-hd4f0392_3.conda");
    }
}
