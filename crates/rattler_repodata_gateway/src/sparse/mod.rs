//! This module provides the [`SparseRepoData`] which is a struct to enable only
//! sparsely loading records from a `repodata.json` file.

#![allow(clippy::mem_forget)]

use std::{
    borrow::Borrow,
    collections::{HashSet, VecDeque},
    fmt, io,
    marker::PhantomData,
    path::Path,
};

use bytes::Bytes;
use fs_err as fs;
use itertools::Itertools;
use rattler_conda_types::{
    compute_package_url, package::ArchiveType, Channel, ChannelInfo, MatchSpec, Matches,
    PackageName, PackageRecord, RepoDataRecord,
};
use rattler_redaction::Redact;
use serde::{
    de::{Error, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::value::RawValue;
use superslice::Ext;
use thiserror::Error;

/// Defines how different variants of packages are consolidated.
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
pub enum PackageFormatSelection {
    /// Only the tar.bz2 packages are used
    OnlyTarBz2,

    /// Only the conda packages are used
    OnlyConda,

    /// Only whl packages are used
    OnlyWhl,

    /// Both .tar.bz2 and .conda packages are used, but if a .conda exists that
    /// represents the same content as a .tar.bz2, the .conda package is
    /// selected and the .tar.bz2 is discarded.
    #[default]
    PreferConda,

    /// Both .tar.bz2 and .conda packages are used
    Both,
}

/// A struct to enable loading records from a `repodata.json` file on demand.
/// Since most of the time you don't need all the records from the
/// `repodata.json` this can help provide some significant speedups.
pub struct SparseRepoData {
    /// Data structure that holds an index into the the records stored in a repo
    /// data.
    inner: SparseRepoDataInner,

    /// The channel from which this data was downloaded.
    channel: Channel,

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
    #[cfg(any(unix, windows))]
    pub fn from_file(
        channel: Channel,
        subdir: impl Into<String>,
        path: impl AsRef<Path>,
        patch_function: Option<fn(&mut PackageRecord)>,
    ) -> Result<Self, io::Error> {
        let file = fs::File::open(path.as_ref().to_owned())?;
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
        let bytes = fs::read(path)?;
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

    /// Returns an iterator over all package names in this repodata file.
    ///
    /// This works by iterating over all elements in the `packages` and
    /// `conda_packages` fields of the repodata and returning the unique
    /// package names.
    pub fn package_names(
        &self,
        package_format_selection: PackageFormatSelection,
    ) -> impl Iterator<Item = &'_ str> {
        fn select_package_name<'i>((filename, _): &(PackageFilename<'i>, &'i RawValue)) -> &'i str {
            filename.package
        }

        let repo_data = self.inner.borrow_repo_data();
        let tar_baz2_packages = repo_data.packages.iter().map(select_package_name);
        let conda_packages = repo_data.conda_packages.iter().map(select_package_name);
        let whl_packages = repo_data.whl_packages.iter().map(select_package_name);

        match package_format_selection {
            PackageFormatSelection::Both | PackageFormatSelection::PreferConda => {
                itertools::Either::Left(
                    tar_baz2_packages.merge(whl_packages).merge(conda_packages).dedup()
                )
            }
            PackageFormatSelection::OnlyTarBz2 => {
                itertools::Either::Right(tar_baz2_packages.dedup())
            }
            PackageFormatSelection::OnlyConda => itertools::Either::Right(conda_packages.dedup()),
            PackageFormatSelection::OnlyWhl => itertools::Either::Right(whl_packages.dedup()),
        }
    }

    /// Returns the number of records in this instance.
    pub fn record_count(&self, package_format_selection: PackageFormatSelection) -> usize {
        match package_format_selection {
            PackageFormatSelection::PreferConda => {
                let repo_data = self.inner.borrow_repo_data();
                let tar_bz2_packages = repo_data.packages.iter().map(|(filename, _)| {
                    filename
                        .filename
                        .strip_suffix(ArchiveType::TarBz2.extension())
                        .unwrap_or(filename.filename)
                });
                let whl_packages = repo_data.whl_packages.iter().map(|(filename, _)| {
                    filename
                        .filename
                        .strip_suffix(ArchiveType::Whl.extension())
                        .unwrap_or(filename.filename)
                });
                let conda_packages = repo_data.conda_packages.iter().map(|(filename, _)| {
                    filename
                        .filename
                        .strip_suffix(ArchiveType::Conda.extension())
                        .unwrap_or(filename.filename)
                });
                conda_packages.merge(tar_bz2_packages).merge(whl_packages).dedup().count()
            }
            PackageFormatSelection::Both => {
                self.inner.borrow_repo_data().packages.len()
                    + self.inner.borrow_repo_data().conda_packages.len()
            }
            PackageFormatSelection::OnlyTarBz2 => self.inner.borrow_repo_data().packages.len(),
            PackageFormatSelection::OnlyConda => self.inner.borrow_repo_data().conda_packages.len(),
            PackageFormatSelection::OnlyWhl => self.inner.borrow_repo_data().whl_packages.len(),
        }
    }

    /// Returns all the records that matches any of the specified match spec.
    pub fn load_matching_records(
        &self,
        spec: impl IntoIterator<Item = impl Borrow<MatchSpec>>,
        variant_consolidation: PackageFormatSelection,
    ) -> io::Result<Vec<RepoDataRecord>> {
        let mut result = Vec::new();
        let repo_data = self.inner.borrow_repo_data();
        let base_url = repo_data.info.as_ref().and_then(|i| i.base_url.as_deref());
        for (package_name, specs) in &spec.into_iter().chunk_by(|spec| spec.borrow().name.clone()) {
            let grouped_specs = specs.into_iter().collect::<Vec<_>>();
            // TODO: support glob/regex package names
            let mut parsed_records = parse_records(
                package_name.and_then(Option::<PackageName>::from).as_ref(),
                &repo_data.packages,
                &repo_data.conda_packages,
                &repo_data.whl_packages,
                variant_consolidation,
                base_url,
                &self.channel,
                &self.subdir,
                self.patch_record_fn,
                |record| {
                    grouped_specs
                        .iter()
                        .any(|spec| spec.borrow().matches(&record.package_record))
                },
            )?;
            result.append(&mut parsed_records);
        }

        Ok(result)
    }

    /// Returns all the records for the specified package name.
    pub fn load_records(
        &self,
        package_name: &PackageName,
        variant_consolidation: PackageFormatSelection,
    ) -> io::Result<Vec<RepoDataRecord>> {
        let repo_data = self.inner.borrow_repo_data();
        let base_url = repo_data.info.as_ref().and_then(|i| i.base_url.as_deref());
        parse_records(
            Some(package_name),
            &repo_data.packages,
            &repo_data.conda_packages,
            &repo_data.whl_packages,
            variant_consolidation,
            base_url,
            &self.channel,
            &self.subdir,
            self.patch_record_fn,
            |_| true, // Dont filter anything out
        )
    }

    /// Returns all the records for the specified package format(s).
    pub fn load_all_records(
        &self,
        variant_consolidation: PackageFormatSelection,
    ) -> io::Result<Vec<RepoDataRecord>> {
        let repo_data = self.inner.borrow_repo_data();
        let base_url = repo_data.info.as_ref().and_then(|i| i.base_url.as_deref());
        parse_records(
            None,
            &repo_data.packages,
            &repo_data.conda_packages,
            &repo_data.whl_packages,
            variant_consolidation,
            base_url,
            &self.channel,
            &self.subdir,
            self.patch_record_fn,
            |_| true,
        )
    }

    /// Given a set of [`SparseRepoData`]s load all the records for the packages
    /// with the specified names and all the packages these records depend
    /// on.
    ///
    /// This will parse the records for the specified packages as well as all
    /// the packages these records depend on.
    pub fn load_records_recursive<'a>(
        repo_data: impl IntoIterator<Item = &'a SparseRepoData>,
        package_names: impl IntoIterator<Item = PackageName>,
        patch_function: Option<fn(&mut PackageRecord)>,
        variant_consolidation: PackageFormatSelection,
    ) -> io::Result<Vec<Vec<RepoDataRecord>>> {
        let repo_data: Vec<_> = repo_data.into_iter().collect();

        // Construct the result map
        let mut result: Vec<_> = (0..repo_data.len()).map(|_| Vec::new()).collect();

        // Construct a set of packages that we have seen and have been added to the
        // pending list.
        let mut seen: HashSet<PackageName> = package_names.into_iter().collect();

        // Construct a queue to store packages in that still need to be processed
        let mut pending: VecDeque<_> = seen.iter().cloned().collect();

        // Iterate over the list of packages that still need to be processed.
        while let Some(next_package) = pending.pop_front() {
            for (i, repo_data) in repo_data.iter().enumerate() {
                let repo_data_packages = repo_data.inner.borrow_repo_data();
                let base_url = repo_data_packages
                    .info
                    .as_ref()
                    .and_then(|i| i.base_url.as_deref());

                // Get all records from the repodata
                let mut records = parse_records(
                    Some(&next_package),
                    &repo_data_packages.packages,
                    &repo_data_packages.conda_packages,
                    &repo_data_packages.whl_packages,
                    variant_consolidation,
                    base_url,
                    &repo_data.channel,
                    &repo_data.subdir,
                    patch_function,
                    |_| true,
                )?;

                // Iterate over all packages to find recursive dependencies.
                for record in records.iter() {
                    for dependency in &record.package_record.depends {
                        let dependency_name = PackageName::from_matchspec_str_unchecked(dependency);
                        if !seen.contains(&dependency_name) {
                            pending.push_back(dependency_name.clone());
                            seen.insert(dependency_name);
                        }
                    }
                }

                result[i].append(&mut records);
            }
        }

        Ok(result)
    }

    /// Returns the subdirectory from which this repodata was loaded
    pub fn subdir(&self) -> &str {
        &self.subdir
    }
}

/// A serde compatible struct that only sparsely parses a repodata.json file.
#[derive(Deserialize)]
struct LazyRepoData<'i> {
    /// The channel information contained in the repodata.json file
    info: Option<ChannelInfo>,

    /// The tar.bz2 packages contained in the repodata.json file
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_filename_and_raw_record"
    )]
    packages: Vec<(PackageFilename<'i>, &'i RawValue)>,

    /// The conda packages contained in the repodata.json file (under a
    /// different key for backwards compatibility with previous conda
    /// versions)
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_filename_and_raw_record",
        rename = "packages.conda"
    )]
    conda_packages: Vec<(PackageFilename<'i>, &'i RawValue)>,

    /// The wheel packages contained in the repodata.json file (under a
    /// different key for to keep wheel and conda packages separate)
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_filename_and_raw_record",
        rename = "packages.whl"
    )]
    whl_packages: Vec<(PackageFilename<'i>, &'i RawValue)>,
}

/// Returns an iterator over the packages in the slice that match the given
/// package name.
fn find_package_in_slice<'a, 'i: 'a>(
    slice: &'a [(PackageFilename<'i>, &'i RawValue)],
    package_name: Option<&PackageName>,
) -> impl Iterator<Item = (PackageFilename<'i>, &'i RawValue)> + 'a {
    let range = match package_name {
        None => 0..slice.len(),
        Some(package_name) => {
            slice.equal_range_by(|(package, _)| package.package.cmp(package_name.as_normalized()))
        }
    };

    slice[range]
        .iter()
        .map(|(filename, raw_json)| (*filename, *raw_json))
}

/// Takes an iterator over package filenames and raw json values and returns an
/// iterator that also includes the filename without an extension.
fn add_stripped_filename<'i>(
    slice: impl Iterator<Item = (PackageFilename<'i>, &'i RawValue)>,
    ext: ArchiveType,
) -> impl Iterator<Item = (PackageFilename<'i>, &'i RawValue, &'i str)> {
    slice.map(move |(filename, raw_json)| {
        (
            filename,
            raw_json,
            filename
                .filename
                .strip_suffix(ext.extension())
                .unwrap_or(filename.filename),
        )
    })
}

/// Parse the records for the specified package from the raw index
#[allow(clippy::too_many_arguments)]
fn parse_records<'i, F: Fn(&RepoDataRecord) -> bool>(
    package_name: Option<&PackageName>,
    tar_bz2_packages: &[(PackageFilename<'i>, &'i RawValue)],
    conda_packages: &[(PackageFilename<'i>, &'i RawValue)],
    whl_packages: &[(PackageFilename<'i>, &'i RawValue)],
    variant_consolidation: PackageFormatSelection,
    base_url: Option<&str>,
    channel: &Channel,
    subdir: &str,
    patch_function: Option<fn(&mut PackageRecord)>,
    filter_function: F,
) -> io::Result<Vec<RepoDataRecord>> {
    match variant_consolidation {
        PackageFormatSelection::PreferConda => {
            let tar_bz2_packages = add_stripped_filename(
                find_package_in_slice(tar_bz2_packages, package_name),
                ArchiveType::TarBz2,
            );
            let whl_packages = add_stripped_filename(
                find_package_in_slice(whl_packages, package_name),
                ArchiveType::Whl,
            );
            let conda_packages = add_stripped_filename(
                find_package_in_slice(conda_packages, package_name),
                ArchiveType::Conda,
            );
            let deduplicated_packages = conda_packages
                // Merge the conda and tar.bz2 packages together based on their filename without
                // extension.
                .merge_by(tar_bz2_packages, |(_, _, left), (_, _, right)| {
                    left <= right
                })
                .merge_by(whl_packages, |(_, _, left), (_, _, right)| {
                    left <= right
                })
                // Deduplicate repeated packages based on their filename without extension. (this
                // removes the .tar.bz2 in favor of the .conda)
                .dedup_by(|(_, _, left), (_, _, right)| left == right)
                .map(|(filename, raw_json, _)| (filename, raw_json));
            parse_records_raw(
                deduplicated_packages,
                base_url,
                channel,
                subdir,
                patch_function,
                filter_function,
            )
        }
        PackageFormatSelection::Both => {
            let tar_bz2_packages = find_package_in_slice(tar_bz2_packages, package_name);
            let whl_packages = find_package_in_slice(whl_packages, package_name);
            let conda_packages = find_package_in_slice(conda_packages, package_name);
            parse_records_raw(
                tar_bz2_packages.chain(conda_packages).chain(whl_packages),
                base_url,
                channel,
                subdir,
                patch_function,
                filter_function,
            )
        }
        PackageFormatSelection::OnlyTarBz2 => {
            let tar_bz2_packages = find_package_in_slice(tar_bz2_packages, package_name);
            parse_records_raw(
                tar_bz2_packages,
                base_url,
                channel,
                subdir,
                patch_function,
                filter_function,
            )
        }
        PackageFormatSelection::OnlyConda => {
            let conda_packages = find_package_in_slice(conda_packages, package_name);
            parse_records_raw(
                conda_packages,
                base_url,
                channel,
                subdir,
                patch_function,
                filter_function,
            )
        }
        PackageFormatSelection::OnlyWhl => {
            let whl_packages= find_package_in_slice(whl_packages, package_name);
            parse_records_raw(
                whl_packages,
                base_url,
                channel,
                subdir,
                patch_function,
                filter_function,
            )
        }
    }
}

fn parse_record_raw<'i>(
    (filename, raw_json): (PackageFilename<'i>, &'i RawValue),
    base_url: Option<&str>,
    channel: &Channel,
    channel_name: Option<String>,
    subdir: &str,
    patch_function: Option<fn(&mut PackageRecord)>,
) -> io::Result<RepoDataRecord> {
    let mut package_record: PackageRecord = serde_json::from_str(raw_json.get())?;
    // Overwrite subdir if its empty
    if package_record.subdir.is_empty() {
        package_record.subdir = subdir.to_owned();
    }
    let mut record = RepoDataRecord {
        url: compute_package_url(
            &channel
                .base_url
                .url()
                .join(&format!("{subdir}/"))
                .expect("failed determine repo_base_url"),
            base_url,
            filename.filename,
        ),
        channel: channel_name.clone(),
        package_record,
        file_name: filename.filename.to_owned(),
    };

    // Apply the patch function if one was specified
    if let Some(patch_fn) = patch_function {
        patch_fn(&mut record.package_record);
    }

    Ok(record)
}

fn parse_records_raw<'i, F: Fn(&RepoDataRecord) -> bool>(
    packages: impl Iterator<Item = (PackageFilename<'i>, &'i RawValue)>,
    base_url: Option<&str>,
    channel: &Channel,
    subdir: &str,
    patch_function: Option<fn(&mut PackageRecord)>,
    filter_function: F,
) -> io::Result<Vec<RepoDataRecord>> {
    let channel_name = channel.base_url.url().clone().redact().to_string();
    packages
        .map(move |record| {
            parse_record_raw(
                record,
                base_url,
                channel,
                Some(channel_name.clone()),
                subdir,
                patch_function,
            )
        })
        .filter_ok(filter_function)
        .collect()
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

fn deserialize_filename_and_raw_record<'d, D: Deserializer<'d>>(
    deserializer: D,
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

    // Although in general the filenames are sorted in repodata.json this doesnt
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
    entries.sort_unstable_by(|(a, _), (b, _)| a.package.cmp(b.package));

    Ok(entries)
}

/// A struct that holds both a filename and the part of the filename that's just
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

    /// The filename is not a valid wheel file
    #[error("filename ({0}) is not a valid .whl file")]
    NotAWheelFile(String),
}

/// Extract package name from a wheel filename per PEP 427.
///
/// Wheel format: {distribution}-{version}(-{build})?-{python}-{abi}-{platform}.whl
///
/// The version component starts with a digit and typically contains dots (e.g., "1.0.0").
/// We find the version by looking for the first dash-separated part that:
/// 1. Starts with a digit, AND
/// 2. Contains a dot OR consists only of digits
///
/// Everything before the version is the package name.
fn extract_wheel_package_name(s: &str) -> Result<&str, PackageFilenameError> {
    // Remove .whl extension
    let name_without_ext = s
        .strip_suffix(".whl")
        .ok_or_else(|| PackageFilenameError::NotAWheelFile(s.to_string()))?;

    // Find dash positions for efficient slicing
    let mut dash_positions = Vec::new();
    for (i, c) in name_without_ext.char_indices() {
        if c == '-' {
            dash_positions.push(i);
        }
    }

    if dash_positions.is_empty() {
        return Err(PackageFilenameError::NotEnoughDashes(s.to_string()));
    }

    // Check each part between dashes to find version start
    let mut start = 0;
    for (part_idx, &dash_pos) in dash_positions.iter().enumerate() {
        let part = &name_without_ext[start..dash_pos];

        if !part.is_empty() {
            let first_char = part.chars().next().unwrap();
            if first_char.is_ascii_digit()
                && (part.contains('.') || part.chars().all(|c| c.is_ascii_digit()))
            {
                // Found version! Return everything before this dash
                if part_idx == 0 {
                    return Err(PackageFilenameError::NotEnoughDashes(s.to_string()));
                }
                return Ok(&s[..start - 1]); // -1 to exclude the dash
            }
        }

        start = dash_pos + 1;
    }

    // Check last part (after final dash)
    let last_part = &name_without_ext[start..];
    if !last_part.is_empty() {
        let first_char = last_part.chars().next().unwrap();
        if first_char.is_ascii_digit()
            && (last_part.contains('.') || last_part.chars().all(|c| c.is_ascii_digit()))
        {
            return Ok(&s[..start - 1]);
        }
    }

    // No version found - return first part as fallback
    Ok(&s[..dash_positions[0]])
}

impl<'de> TryFrom<&'de str> for PackageFilename<'de> {
    type Error = PackageFilenameError;

    fn try_from(s: &'de str) -> Result<Self, Self::Error> {
        let package = if s.ends_with(".whl") {
            // Wheel format: {distribution}-{version}(-{build})?-{python}-{abi}-{platform}.whl
            // Extract package name by finding where version starts
            extract_wheel_package_name(s)?
        } else {
            // Conda format: {name}-{version}-{build}.{ext}
            // Extract package name using existing logic
            s.rsplitn(3, '-')
                .nth(2)
                .ok_or(PackageFilenameError::NotEnoughDashes(s.to_string()))?
        };

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
    };
    use rstest::rstest;

    use super::{
        load_repo_data_recursively, PackageFilename, PackageFormatSelection, SparseRepoData,
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

        // Memmapped
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
    // Existing conda test cases
    #[case("clang-format-13.0.1-root_62800_h69bbbaa_1.conda", "clang-format")]
    #[case("clang-format-13-13.0.1-default_he082bbe_0.tar.bz2", "clang-format-13")]
    // New wheel test cases - standard packages
    #[case("requests-2.32.5-py3-none-any.whl", "requests")]
    #[case("typing_extensions-4.14.1-py3-none-any.whl", "typing_extensions")]
    #[case("pydantic_core-2.33.2-cp313-cp313-macosx_11_0_arm64.whl", "pydantic_core")]
    #[case("numpy-1.24.3-cp39-cp39-win_amd64.whl", "numpy")]
    // Edge cases - hyphenated package names
    #[case("scikit-learn-1.3.0-cp311-cp311-linux_x86_64.whl", "scikit-learn")]
    #[case("foo-bar-baz-1.0.0-py3-none-any.whl", "foo-bar-baz")]
    // Edge cases - underscores in names
    #[case("package_with_underscore-2.0.0-py3-none-any.whl", "package_with_underscore")]
    // Edge cases - single digit versions
    #[case("simple-1-py3-none-any.whl", "simple")]
    #[case("simple-10-py3-none-any.whl", "simple")]
    fn test_deserialize_package_name(#[case] filename: &str, #[case] result: &str) {
        assert_eq!(PackageFilename::try_from(filename).unwrap().package, result);
    }

    #[test]
    fn test_wheel_package_name_extraction_errors() {
        // Malformed wheel - no version
        assert!(PackageFilename::try_from("malformed.whl").is_err());

        // Malformed wheel - version is first part
        assert!(PackageFilename::try_from("1.0.0-py3-none-any.whl").is_err());

        // Not enough dashes
        assert!(PackageFilename::try_from("package.whl").is_err());
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

    #[rstest]
    #[case::both(PackageFormatSelection::Both)]
    #[case::prefer_conda(PackageFormatSelection::PreferConda)]
    #[case::only_tar_bz2(PackageFormatSelection::OnlyTarBz2)]
    #[case::only_conda(PackageFormatSelection::OnlyConda)]
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
    #[case::only_tar_bz2(PackageFormatSelection::OnlyTarBz2)]
    #[case::only_conda(PackageFormatSelection::OnlyConda)]
    fn test_package_format_selection(#[case] variant: PackageFormatSelection) {
        let (channel, platform, path) = dummy_repo_data();
        let sparse = SparseRepoData::from_file(channel, platform, path, None).unwrap();
        let records = sparse
            .load_records(&PackageName::try_from("bors").unwrap(), variant)
            .unwrap()
            .into_iter()
            .map(|record| record.file_name)
            .collect::<Vec<_>>();

        insta::with_settings!({snapshot_suffix => variant.to_string()}, {
            insta::assert_snapshot!(records.join("\n"));
        });
    }

    #[rstest]
    #[case::both(PackageFormatSelection::Both, 29)]
    #[case::prefer_conda(PackageFormatSelection::PreferConda, 25)]
    #[case::only_tar_bz2(PackageFormatSelection::OnlyTarBz2, 24)]
    #[case::only_conda(PackageFormatSelection::OnlyConda, 5)]
    fn test_record_count(#[case] variant: PackageFormatSelection, #[case] expected_count: usize) {
        let (channel, platform, path) = dummy_repo_data();
        let sparse = SparseRepoData::from_file(channel, platform, path, None).unwrap();
        let count = sparse.record_count(variant);
        assert_eq!(count, expected_count);
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
            .map(|record| record.file_name)
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
                vec![MatchSpec::from_str("* 12.5", ParseStrictness::Lenient).unwrap()],
                PackageFormatSelection::default(),
            )
            .unwrap()
            .into_iter()
            .map(|record| record.file_name)
            .collect::<Vec<_>>();

        insta::assert_snapshot!(records.join("\n"), @"cuda-version-12.5-hd4f0392_3.conda");
    }

    #[test]
    fn test_wheel_package_loading() {
        // Create test repodata with wheel packages
        let json = r#"{
            "info": {
                "subdir": "noarch"
            },
            "packages": {},
            "packages.conda": {},
            "packages.whl": {
                "requests-2.32.5-py3-none-any.whl": {
                    "name": "requests",
                    "version": "2.32.5",
                    "build": "py3_0",
                    "build_number": 0,
                    "depends": ["charset-normalizer <4,>=2"],
                    "subdir": "noarch",
                    "md5": "2462f94637a34fd532264295e186976d",
                    "sha256": "2462f94637a34fd532264295e186976db0f5d453d1cdd31473c85a6a161affb6",
                    "size": 64738,
                    "timestamp": 1764005009
                },
                "typing_extensions-4.14.1-py3-none-any.whl": {
                    "name": "typing_extensions",
                    "version": "4.14.1",
                    "build": "py3_0",
                    "build_number": 0,
                    "depends": [],
                    "subdir": "noarch",
                    "md5": "d1e1e3b58374dc93031d6eda2420a48e",
                    "sha256": "d1e1e3b58374dc93031d6eda2420a48ea44a36c2b4766a4fdeb3710755731d76",
                    "size": 43906,
                    "timestamp": 1756405213
                },
                "scikit-learn-1.3.0-cp311-cp311-linux_x86_64.whl": {
                    "name": "scikit-learn",
                    "version": "1.3.0",
                    "build": "cp311_0",
                    "build_number": 0,
                    "depends": ["numpy"],
                    "subdir": "linux-64",
                    "md5": "97ec377d2ad83dfef1194b7aa31b0c90",
                    "sha256": "97ec377d2ad83dfef1194b7aa31b0c9076194e10d995a6e696c9d07dd782b14a",
                    "size": 414494,
                    "timestamp": 1715610974
                }
            }
        }"#;

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

        // Test that package names are correctly extracted
        let names: Vec<_> = sparse_repodata
            .package_names(PackageFormatSelection::OnlyWhl)
            .collect();

        assert_eq!(names, vec!["requests", "scikit-learn", "typing_extensions"]);

        // Test loading records by name
        let records = sparse_repodata
            .load_records(
                &PackageName::try_from("requests").unwrap(),
                PackageFormatSelection::OnlyWhl,
            )
            .unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].file_name, "requests-2.32.5-py3-none-any.whl");
        assert_eq!(records[0].package_record.name.as_normalized(), "requests");
        assert_eq!(records[0].package_record.version.to_string(), "2.32.5");

        // Test loading records with hyphenated package name
        let records = sparse_repodata
            .load_records(
                &PackageName::try_from("scikit-learn").unwrap(),
                PackageFormatSelection::OnlyWhl,
            )
            .unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].file_name,
            "scikit-learn-1.3.0-cp311-cp311-linux_x86_64.whl"
        );

        // Test loading all wheel records
        let all_records = sparse_repodata
            .load_all_records(PackageFormatSelection::OnlyWhl)
            .unwrap();

        assert_eq!(all_records.len(), 3);

        // Test record count
        assert_eq!(
            sparse_repodata.record_count(PackageFormatSelection::OnlyWhl),
            3
        );
    }

    #[test]
    #[ignore] // Only run manually when the external file exists
    fn test_actual_wheel_repodata_from_conda_pypi() {
        use std::path::PathBuf;

        let path = PathBuf::from("/Users/travishathaway/dev/conda-pypi/tests/conda_local_channel/noarch/repodata.json");

        if !path.exists() {
            // Skip test if file doesn't exist
            return;
        }

        let channel = Channel::from_str(
            "test-channel",
            &ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap()),
        )
        .unwrap();

        let sparse = SparseRepoData::from_file(channel, "noarch", &path, None).unwrap();

        // Test extracting package names
        let package_names: Vec<_> = sparse.package_names(PackageFormatSelection::OnlyWhl).collect();
        assert!(!package_names.is_empty(), "Should find wheel packages");

        // Verify some expected packages
        assert!(package_names.contains(&"requests"));
        assert!(package_names.contains(&"typing_extensions") || package_names.contains(&"typing-extensions"));

        // Test loading a specific package
        let records = sparse
            .load_records(
                &PackageName::try_from("requests").unwrap(),
                PackageFormatSelection::OnlyWhl,
            )
            .unwrap();

        assert!(!records.is_empty(), "Should load requests package");
        assert!(records[0].file_name.ends_with(".whl"));

        // Test PreferConda mode includes wheels
        let all_records = sparse.load_all_records(PackageFormatSelection::PreferConda).unwrap();
        assert!(!all_records.is_empty(), "PreferConda should include wheel packages");
    }
}
