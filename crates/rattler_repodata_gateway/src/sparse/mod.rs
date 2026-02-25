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
    compute_package_url,
    package::{
        ArchiveIdentifier, CondaArchiveType, DistArchiveIdentifier, DistArchiveType,
        WheelArchiveType,
    },
    Channel, ChannelInfo, MatchSpec, Matches, PackageName, PackageRecord, RepoDataRecord,
    UrlOrPath, WhlPackageRecord,
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

    /// Both .tar.bz2 and .conda packages are used, but if a .conda exists that
    /// represents the same content as a .tar.bz2, the .conda package is
    /// selected and the .tar.bz2 is discarded.
    #[default]
    PreferConda,

    /// .tar.bz2, .conda and .whl packages are used, but if a .conda exists that
    /// represents the same content as a .tar.bz2 or .whl, the .conda package is
    /// selected and the .tar.bz2 is discarded.
    PreferCondaWithWhl,

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
        let tar_bz2_packages = repo_data.packages.iter().map(select_package_name);
        let conda_packages = repo_data.conda_packages.iter().map(select_package_name);
        let v3_tar = repo_data
            .experimental_v3
            .tar_bz2
            .iter()
            .map(select_package_name);
        let v3_conda = repo_data
            .experimental_v3
            .conda
            .iter()
            .map(select_package_name);
        let v3_whl = repo_data
            .experimental_v3
            .whl
            .iter()
            .map(select_package_name);

        match package_format_selection {
            PackageFormatSelection::Both | PackageFormatSelection::PreferConda => {
                itertools::Either::Left(itertools::Either::Left(
                    tar_bz2_packages
                        .merge(v3_tar)
                        .merge(conda_packages.merge(v3_conda))
                        .dedup(),
                ))
            }
            PackageFormatSelection::PreferCondaWithWhl => {
                itertools::Either::Left(itertools::Either::Right(
                    tar_bz2_packages
                        .merge(v3_tar)
                        .merge(v3_whl)
                        .merge(conda_packages.merge(v3_conda))
                        .dedup(),
                ))
            }
            PackageFormatSelection::OnlyTarBz2 => itertools::Either::Right(
                itertools::Either::Left(tar_bz2_packages.merge(v3_tar).dedup()),
            ),
            PackageFormatSelection::OnlyConda => itertools::Either::Right(
                itertools::Either::Right(conda_packages.merge(v3_conda).dedup()),
            ),
        }
    }

    /// Returns the number of records in this instance.
    pub fn record_count(&self, package_format_selection: PackageFormatSelection) -> usize {
        let repo_data = self.inner.borrow_repo_data();
        match package_format_selection {
            PackageFormatSelection::PreferConda | PackageFormatSelection::PreferCondaWithWhl => {
                let tar_bz2_packages = repo_data.packages.iter().map(|(filename, _)| {
                    filename
                        .filename
                        .strip_suffix(CondaArchiveType::TarBz2.extension())
                        .unwrap_or(filename.filename)
                });
                let v3_tar = repo_data
                    .experimental_v3
                    .tar_bz2
                    .iter()
                    .map(|(filename, _)| filename.filename);
                let conda_packages = repo_data.conda_packages.iter().map(|(filename, _)| {
                    filename
                        .filename
                        .strip_suffix(CondaArchiveType::Conda.extension())
                        .unwrap_or(filename.filename)
                });
                let v3_conda = repo_data
                    .experimental_v3
                    .conda
                    .iter()
                    .map(|(filename, _)| filename.filename);

                if package_format_selection == PackageFormatSelection::PreferCondaWithWhl {
                    let v3_whl = repo_data
                        .experimental_v3
                        .whl
                        .iter()
                        .map(|(filename, _)| filename.filename);
                    conda_packages
                        .merge(v3_conda)
                        .merge(tar_bz2_packages.merge(v3_tar))
                        .merge(v3_whl)
                        .dedup()
                        .count()
                } else {
                    conda_packages
                        .merge(v3_conda)
                        .merge(tar_bz2_packages.merge(v3_tar))
                        .dedup()
                        .count()
                }
            }
            PackageFormatSelection::Both => {
                repo_data.packages.len()
                    + repo_data.conda_packages.len()
                    + repo_data.experimental_v3.tar_bz2.len()
                    + repo_data.experimental_v3.conda.len()
            }
            PackageFormatSelection::OnlyTarBz2 => {
                repo_data.packages.len() + repo_data.experimental_v3.tar_bz2.len()
            }
            PackageFormatSelection::OnlyConda => {
                repo_data.conda_packages.len() + repo_data.experimental_v3.conda.len()
            }
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
                &repo_data.experimental_v3,
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
            &repo_data.experimental_v3,
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
            &repo_data.experimental_v3,
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
                    &repo_data_packages.experimental_v3,
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

    /// Packages stored under the `v3` top-level key.
    #[serde(borrow, default, rename = "v3")]
    experimental_v3: LazyV3Packages<'i>,
}

/// Lazily parsed `v3` section of repodata containing sub-maps for each archive
/// type.
#[derive(Deserialize, Default)]
struct LazyV3Packages<'i> {
    /// v3 tar.bz2 packages
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_filename_and_raw_record",
        rename = "tar.bz2"
    )]
    tar_bz2: Vec<(PackageFilename<'i>, &'i RawValue)>,

    /// v3 conda packages
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_filename_and_raw_record"
    )]
    conda: Vec<(PackageFilename<'i>, &'i RawValue)>,

    /// v3 whl packages
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_filename_and_raw_record"
    )]
    whl: Vec<(PackageFilename<'i>, &'i RawValue)>,
}

/// Defines the type of record
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordKind {
    /// This is a regular `packages` or `packages.conda` record.
    CondaOrTarBz2,
    /// This is a `v3.tar.bz2` record (extension-less key).
    V3TarBz2,
    /// This is a `v3.conda` record (extension-less key).
    V3Conda,
    /// This is a `v3.whl` record (extension-less key).
    V3Whl,
}

/// Returns an iterator over the packages in the slice that match the given
/// package name.
fn find_package_in_slice<'a, 'i: 'a>(
    slice: &'a [(PackageFilename<'i>, &'i RawValue)],
    package_name: Option<&PackageName>,
    record_kind: RecordKind,
) -> impl Iterator<Item = (PackageFilename<'i>, &'i RawValue, RecordKind)> + 'a {
    let range = match package_name {
        None => 0..slice.len(),
        Some(package_name) => {
            slice.equal_range_by(|(package, _)| package.package.cmp(package_name.as_normalized()))
        }
    };

    slice[range]
        .iter()
        .map(move |(filename, raw_json)| (*filename, *raw_json, record_kind))
}

/// Takes an iterator over package filenames and raw json values and returns an
/// iterator that also includes the filename without an extension.
fn add_stripped_filename<'i>(
    slice: impl Iterator<Item = (PackageFilename<'i>, &'i RawValue, RecordKind)>,
    ext: DistArchiveType,
) -> impl Iterator<Item = (PackageFilename<'i>, &'i RawValue, RecordKind, &'i str)> {
    slice.map(move |(filename, raw_json, record_kind)| {
        (
            filename,
            raw_json,
            record_kind,
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
    v3: &LazyV3Packages<'i>,
    variant_consolidation: PackageFormatSelection,
    base_url: Option<&str>,
    channel: &Channel,
    subdir: &str,
    patch_function: Option<fn(&mut PackageRecord)>,
    filter_function: F,
) -> io::Result<Vec<RepoDataRecord>> {
    match variant_consolidation {
        PackageFormatSelection::PreferConda => {
            let tar_bz2 = add_stripped_filename(
                find_package_in_slice(tar_bz2_packages, package_name, RecordKind::CondaOrTarBz2),
                DistArchiveType::from(CondaArchiveType::TarBz2),
            );
            let v3_tar = add_stripped_filename(
                find_package_in_slice(&v3.tar_bz2, package_name, RecordKind::V3TarBz2),
                DistArchiveType::from(CondaArchiveType::TarBz2),
            );
            let all_tar = tar_bz2.merge_by(v3_tar, |(_, _, _, a), (_, _, _, b)| a <= b);

            let conda = add_stripped_filename(
                find_package_in_slice(conda_packages, package_name, RecordKind::CondaOrTarBz2),
                DistArchiveType::from(CondaArchiveType::Conda),
            );
            let v3_conda_iter = add_stripped_filename(
                find_package_in_slice(&v3.conda, package_name, RecordKind::V3Conda),
                DistArchiveType::from(CondaArchiveType::Conda),
            );
            let all_conda = conda.merge_by(v3_conda_iter, |(_, _, _, a), (_, _, _, b)| a <= b);

            let deduplicated_packages = all_conda
                .merge_by(all_tar, |(_, _, _, left), (_, _, _, right)| left <= right)
                .dedup_by(|(_, _, _, left), (_, _, _, right)| left == right)
                .map(|(filename, raw_json, record_kind, _)| (filename, raw_json, record_kind));
            parse_records_raw(
                deduplicated_packages,
                base_url,
                channel,
                subdir,
                patch_function,
                filter_function,
            )
        }
        PackageFormatSelection::PreferCondaWithWhl => {
            let tar_bz2 = add_stripped_filename(
                find_package_in_slice(tar_bz2_packages, package_name, RecordKind::CondaOrTarBz2),
                DistArchiveType::from(CondaArchiveType::TarBz2),
            );
            let v3_tar = add_stripped_filename(
                find_package_in_slice(&v3.tar_bz2, package_name, RecordKind::V3TarBz2),
                DistArchiveType::from(CondaArchiveType::TarBz2),
            );
            let all_tar = tar_bz2.merge_by(v3_tar, |(_, _, _, a), (_, _, _, b)| a <= b);

            let whl = add_stripped_filename(
                find_package_in_slice(&v3.whl, package_name, RecordKind::V3Whl),
                DistArchiveType::from(WheelArchiveType::Whl),
            );

            let conda = add_stripped_filename(
                find_package_in_slice(conda_packages, package_name, RecordKind::CondaOrTarBz2),
                DistArchiveType::from(CondaArchiveType::Conda),
            );
            let v3_conda_iter = add_stripped_filename(
                find_package_in_slice(&v3.conda, package_name, RecordKind::V3Conda),
                DistArchiveType::from(CondaArchiveType::Conda),
            );
            let all_conda = conda.merge_by(v3_conda_iter, |(_, _, _, a), (_, _, _, b)| a <= b);

            let deduplicated_packages = all_conda
                .merge_by(whl, |(_, _, _, left), (_, _, _, right)| left <= right)
                .merge_by(all_tar, |(_, _, _, left), (_, _, _, right)| left <= right)
                .dedup_by(|(_, _, _, left), (_, _, _, right)| left == right)
                .map(|(filename, raw_json, kind, _)| (filename, raw_json, kind));
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
            let tar_bz2 =
                find_package_in_slice(tar_bz2_packages, package_name, RecordKind::CondaOrTarBz2);
            let conda =
                find_package_in_slice(conda_packages, package_name, RecordKind::CondaOrTarBz2);
            let v3_tar = find_package_in_slice(&v3.tar_bz2, package_name, RecordKind::V3TarBz2);
            let v3_conda_iter = find_package_in_slice(&v3.conda, package_name, RecordKind::V3Conda);
            parse_records_raw(
                tar_bz2.chain(conda).chain(v3_tar).chain(v3_conda_iter),
                base_url,
                channel,
                subdir,
                patch_function,
                filter_function,
            )
        }
        PackageFormatSelection::OnlyTarBz2 => {
            let tar_bz2 =
                find_package_in_slice(tar_bz2_packages, package_name, RecordKind::CondaOrTarBz2);
            let v3_tar = find_package_in_slice(&v3.tar_bz2, package_name, RecordKind::V3TarBz2);
            parse_records_raw(
                tar_bz2.chain(v3_tar),
                base_url,
                channel,
                subdir,
                patch_function,
                filter_function,
            )
        }
        PackageFormatSelection::OnlyConda => {
            let conda =
                find_package_in_slice(conda_packages, package_name, RecordKind::CondaOrTarBz2);
            let v3_conda_iter = find_package_in_slice(&v3.conda, package_name, RecordKind::V3Conda);
            parse_records_raw(
                conda.chain(v3_conda_iter),
                base_url,
                channel,
                subdir,
                patch_function,
                filter_function,
            )
        }
    }
}

/// Wheel and conda package records are very similar except for how the URL
/// is stored. This function parses a conda package record from its raw json
/// representation.
fn parse_conda_record_raw<'i>(
    filename: PackageFilename<'i>,
    raw_json: &'i RawValue,
    base_url: Option<&str>,
    channel: &Channel,
    channel_name: Option<String>,
    subdir: &str,
) -> io::Result<RepoDataRecord> {
    let mut package_record: PackageRecord = serde_json::from_str(raw_json.get())?;
    // Overwrite subdir if its empty
    if package_record.subdir.is_empty() {
        package_record.subdir = subdir.to_owned();
    }
    let identifier: DistArchiveIdentifier = filename.filename.parse().map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid archive identifier '{}': {}", filename.filename, e),
        )
    })?;
    Ok(RepoDataRecord {
        url: compute_package_url(
            &channel
                .base_url
                .url()
                .join(&format!("{subdir}/"))
                .expect("failed determine repo_base_url"),
            base_url,
            &identifier.to_string(),
        ),
        channel: channel_name,
        package_record,
        identifier,
    })
}

/// Parses a v3 conda/tar.bz2 record from its raw json representation.
/// The key is an extension-less `ArchiveIdentifier`.
fn parse_v3_conda_record_raw<'i>(
    filename: PackageFilename<'i>,
    raw_json: &'i RawValue,
    archive_type: DistArchiveType,
    base_url: Option<&str>,
    channel: &Channel,
    channel_name: Option<String>,
    subdir: &str,
) -> io::Result<RepoDataRecord> {
    let mut package_record: PackageRecord = serde_json::from_str(raw_json.get())?;
    if package_record.subdir.is_empty() {
        package_record.subdir = subdir.to_owned();
    }
    let archive_id: ArchiveIdentifier = filename.filename.parse().map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid archive identifier '{}': {}", filename.filename, e),
        )
    })?;
    let identifier = DistArchiveIdentifier::new(archive_id, archive_type);
    Ok(RepoDataRecord {
        url: compute_package_url(
            &channel
                .base_url
                .url()
                .join(&format!("{subdir}/"))
                .expect("failed determine repo_base_url"),
            base_url,
            &identifier.to_file_name(),
        ),
        channel: channel_name,
        package_record,
        identifier,
    })
}

/// Parses a v3 whl record from its raw json representation.
/// The key is an extension-less `ArchiveIdentifier`.
fn parse_v3_whl_record_raw<'i>(
    filename: PackageFilename<'i>,
    raw_json: &'i RawValue,
    base_url: Option<&str>,
    channel: &Channel,
    channel_name: Option<String>,
    subdir: &str,
) -> io::Result<RepoDataRecord> {
    let WhlPackageRecord {
        url,
        mut package_record,
    } = serde_json::from_str(raw_json.get())?;
    if package_record.subdir.is_empty() {
        package_record.subdir = subdir.to_owned();
    }
    let archive_id: ArchiveIdentifier = filename.filename.parse().map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid archive identifier '{}': {}", filename.filename, e),
        )
    })?;
    let identifier = DistArchiveIdentifier::new(archive_id, WheelArchiveType::Whl);
    let url = match url {
        UrlOrPath::Path(path) => compute_package_url(
            &channel
                .base_url
                .url()
                .join(&format!("{subdir}/"))
                .expect("failed determine repo_base_url"),
            base_url,
            &path,
        ),
        UrlOrPath::Url(url) => url,
    };

    Ok(RepoDataRecord {
        url,
        channel: channel_name,
        package_record,
        identifier,
    })
}

fn parse_record_raw<'i>(
    (filename, raw_json, kind): (PackageFilename<'i>, &'i RawValue, RecordKind),
    base_url: Option<&str>,
    channel: &Channel,
    channel_name: Option<String>,
    subdir: &str,
    patch_function: Option<fn(&mut PackageRecord)>,
) -> io::Result<RepoDataRecord> {
    let mut record = match kind {
        RecordKind::CondaOrTarBz2 => {
            parse_conda_record_raw(filename, raw_json, base_url, channel, channel_name, subdir)?
        }
        RecordKind::V3TarBz2 => parse_v3_conda_record_raw(
            filename,
            raw_json,
            CondaArchiveType::TarBz2.into(),
            base_url,
            channel,
            channel_name,
            subdir,
        )?,
        RecordKind::V3Conda => parse_v3_conda_record_raw(
            filename,
            raw_json,
            CondaArchiveType::Conda.into(),
            base_url,
            channel,
            channel_name,
            subdir,
        )?,
        RecordKind::V3Whl => {
            parse_v3_whl_record_raw(filename, raw_json, base_url, channel, channel_name, subdir)?
        }
    };

    // Apply the patch function if one was specified
    if let Some(patch_fn) = patch_function {
        patch_fn(&mut record.package_record);
    }

    Ok(record)
}

fn parse_records_raw<'i, F: Fn(&RepoDataRecord) -> bool>(
    packages: impl Iterator<Item = (PackageFilename<'i>, &'i RawValue, RecordKind)>,
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
    entries.sort_unstable_by(|(a, _), (b, _)| a.package.cmp(b.package));

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

    #[rstest]
    #[case::both(PackageFormatSelection::Both)]
    #[case::prefer_conda(PackageFormatSelection::PreferConda)]
    #[case::prefer_conda_with_whl(PackageFormatSelection::PreferCondaWithWhl)]
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
    #[case::prefer_conda_with_whl(PackageFormatSelection::PreferCondaWithWhl)]
    #[case::only_tar_bz2(PackageFormatSelection::OnlyTarBz2)]
    #[case::only_conda(PackageFormatSelection::OnlyConda)]
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
    fn test_record_count(#[case] variant: PackageFormatSelection, #[case] expected_count: usize) {
        let (channel, platform, path) = dummy_repo_data();
        let sparse = SparseRepoData::from_file(channel, platform, path, None).unwrap();
        let count = sparse.record_count(variant);
        assert_eq!(count, expected_count);
    }

    #[rstest]
    #[case::both(PackageFormatSelection::Both, 6)]
    #[case::prefer_conda(PackageFormatSelection::PreferConda, 6)]
    #[case::prefer_conda_with_whl(PackageFormatSelection::PreferCondaWithWhl, 51)]
    #[case::only_tar_bz2(PackageFormatSelection::OnlyTarBz2, 3)]
    #[case::only_conda(PackageFormatSelection::OnlyConda, 3)]
    fn test_record_count_with_wheels(
        #[case] variant: PackageFormatSelection,
        #[case] expected_count: usize,
    ) {
        let (channel, platform, path) = wheel_repo_data();
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
                vec![MatchSpec::from_str("* 12.5", ParseStrictness::Lenient).unwrap()],
                PackageFormatSelection::default(),
            )
            .unwrap()
            .into_iter()
            .map(|record| record.identifier.to_file_name())
            .collect::<Vec<_>>();

        insta::assert_snapshot!(records.join("\n"), @"cuda-version-12.5-hd4f0392_3.conda");
    }
}
