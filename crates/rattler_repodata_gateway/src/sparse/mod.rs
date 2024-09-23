//! This module provides the [`SparseRepoData`] which is a struct to enable only
//! sparsely loading records from a `repodata.json` file.

#![allow(clippy::mem_forget)]

use std::{
    collections::{HashSet, VecDeque},
    fmt, io,
    marker::PhantomData,
    path::Path,
};

use bytes::Bytes;
use fs_err as fs;
use futures::{stream, StreamExt, TryFutureExt, TryStreamExt};
use itertools::Itertools;
use rattler_conda_types::{
    compute_package_url, Channel, ChannelInfo, PackageName, PackageRecord, RepoDataRecord,
};
use serde::{
    de::{Error, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::value::RawValue;
use superslice::Ext;
use thiserror::Error;

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
    Memmapped(MemmappedSparseRepoDataInner),
    /// The repo data is stored as `Bytes`
    Bytes(BytesSparseRepoDataInner),
}

impl SparseRepoDataInner {
    fn borrow_repo_data(&self) -> &LazyRepoData<'_> {
        match self {
            SparseRepoDataInner::Memmapped(inner) => inner.borrow_repo_data(),
            SparseRepoDataInner::Bytes(inner) => inner.borrow_repo_data(),
        }
    }
}

/// A struct that holds a memory map of a `repodata.json` file and also a
/// self-referential field which indexes the data in the memory map with a
/// sparsely parsed json struct. See [`LazyRepoData`].
#[ouroboros::self_referencing]
struct MemmappedSparseRepoDataInner {
    /// Memory map of the `repodata.json` file
    memory_map: memmap2::Mmap,

    /// Sparsely parsed json content of the memory map. This data struct holds
    /// references into the memory map so we have to use ouroboros to make
    /// this legal.
    #[borrows(memory_map)]
    #[covariant]
    repo_data: LazyRepoData<'this>,
}

/// A struct that holds a reference to the bytes of a `repodata.json` file and
/// also a self-referential field which indexes the data in the `bytes` with a
/// sparsely parsed json struct. See [`LazyRepoData`].
#[ouroboros::self_referencing]
struct BytesSparseRepoDataInner {
    /// Bytes of the `repodata.json` file
    bytes: Bytes,

    /// Sparsely parsed json content of the file's bytes. This data struct holds
    /// references into the bytes so we have to use ouroboros to make this
    /// legal.
    #[borrows(bytes)]
    #[covariant]
    repo_data: LazyRepoData<'this>,
}

impl SparseRepoData {
    /// Construct an instance of self from a file on disk and a [`Channel`].
    ///
    /// The `patch_function` can be used to patch the package record after it
    /// has been parsed (e.g. to add `pip` to `python`).
    pub fn new(
        channel: Channel,
        subdir: impl Into<String>,
        path: impl AsRef<Path>,
        patch_function: Option<fn(&mut PackageRecord)>,
    ) -> Result<Self, io::Error> {
        let file = fs::File::open(path.as_ref().to_owned())?;
        let memory_map = unsafe { memmap2::Mmap::map(&file) }?;
        Ok(SparseRepoData {
            inner: SparseRepoDataInner::Memmapped(
                MemmappedSparseRepoDataInnerTryBuilder {
                    memory_map,
                    repo_data_builder: |memory_map| serde_json::from_slice(memory_map.as_ref()),
                }
                .try_build()?,
            ),
            subdir: subdir.into(),
            channel,
            patch_record_fn: patch_function,
        })
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
            inner: SparseRepoDataInner::Bytes(
                BytesSparseRepoDataInnerTryBuilder {
                    bytes,
                    repo_data_builder: |bytes| serde_json::from_slice(bytes),
                }
                .try_build()?,
            ),
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
    pub fn package_names(&self) -> impl Iterator<Item = &'_ str> + '_ {
        let repo_data = self.inner.borrow_repo_data();
        repo_data
            .packages
            .iter()
            .chain(repo_data.conda_packages.iter())
            .map(|(name, _)| name.package)
            .dedup()
    }

    /// Returns all the records for the specified package name.
    pub fn load_records(&self, package_name: &PackageName) -> io::Result<Vec<RepoDataRecord>> {
        let repo_data = self.inner.borrow_repo_data();
        let base_url = repo_data.info.as_ref().and_then(|i| i.base_url.as_deref());
        let mut records = parse_records(
            package_name,
            &repo_data.packages,
            base_url,
            &self.channel,
            &self.subdir,
            self.patch_record_fn,
        )?;
        let mut conda_records = parse_records(
            package_name,
            &repo_data.conda_packages,
            base_url,
            &self.channel,
            &self.subdir,
            self.patch_record_fn,
        )?;
        records.append(&mut conda_records);
        Ok(records)
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
                    &next_package,
                    &repo_data_packages.packages,
                    base_url,
                    &repo_data.channel,
                    &repo_data.subdir,
                    patch_function,
                )?;
                let mut conda_records = parse_records(
                    &next_package,
                    &repo_data_packages.conda_packages,
                    base_url,
                    &repo_data.channel,
                    &repo_data.subdir,
                    patch_function,
                )?;
                records.append(&mut conda_records);

                // Iterate over all packages to find recursive dependencies.
                for record in records.iter() {
                    for dependency in &record.package_record.depends {
                        let dependency_name = PackageName::new_unchecked(
                            dependency.split_once(' ').unwrap_or((dependency, "")).0,
                        );
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
}

/// Parse the records for the specified package from the raw index
fn parse_records<'i>(
    package_name: &PackageName,
    packages: &[(PackageFilename<'i>, &'i RawValue)],
    base_url: Option<&str>,
    channel: &Channel,
    subdir: &str,
    patch_function: Option<fn(&mut PackageRecord)>,
) -> io::Result<Vec<RepoDataRecord>> {
    let channel_name = channel.canonical_name();

    let package_indices =
        packages.equal_range_by(|(package, _)| package.package.cmp(package_name.as_normalized()));
    let mut result = Vec::with_capacity(package_indices.len());
    for (key, raw_json) in &packages[package_indices] {
        let mut package_record: PackageRecord = serde_json::from_str(raw_json.get())?;
        // Overwrite subdir if its empty
        if package_record.subdir.is_empty() {
            package_record.subdir = subdir.to_owned();
        }
        result.push(RepoDataRecord {
            url: compute_package_url(
                &channel
                    .base_url
                    .join(&format!("{}/", &package_record.subdir))
                    .expect("failed determine repo_base_url"),
                base_url,
                key.filename,
            ),
            channel: channel_name.clone(),
            package_record,
            file_name: key.filename.to_owned(),
        });
    }

    // Apply the patch function if one was specified
    if let Some(patch_fn) = patch_function {
        for record in &mut result {
            patch_fn(&mut record.package_record);
        }
    }

    Ok(result)
}

/// A helper function that immediately loads the records for the given packages
/// (and their dependencies). Records for the specified packages are loaded from
/// the repodata files. The `patch_record_fn` is applied to each record after it
/// has been parsed and can mutate the record after it has been loaded.
pub async fn load_repo_data_recursively(
    repo_data_paths: impl IntoIterator<Item = (Channel, impl Into<String>, impl AsRef<Path>)>,
    package_names: impl IntoIterator<Item = PackageName>,
    patch_function: Option<fn(&mut PackageRecord)>,
) -> Result<Vec<Vec<RepoDataRecord>>, io::Error> {
    // Open the different files and memory map them to get access to their bytes. Do
    // this in parallel.
    let lazy_repo_data = stream::iter(repo_data_paths)
        .map(|(channel, subdir, path)| {
            let path = path.as_ref().to_path_buf();
            let subdir = subdir.into();
            tokio::task::spawn_blocking(move || {
                SparseRepoData::new(channel, subdir, path, patch_function)
            })
            .unwrap_or_else(|r| match r.try_into_panic() {
                Ok(panic) => std::panic::resume_unwind(panic),
                Err(err) => Err(io::Error::new(io::ErrorKind::Other, err.to_string())),
            })
        })
        .buffered(50)
        .try_collect::<Vec<_>>()
        .await?;

    SparseRepoData::load_records_recursive(&lazy_repo_data, package_names, patch_function)
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

/// A struct that holds both a filename and the part of the filename thats just
/// the package name.
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
    use std::path::{Path, PathBuf};

    use bytes::Bytes;
    use itertools::Itertools;
    use rattler_conda_types::{Channel, ChannelConfig, PackageName, RepoData, RepoDataRecord};
    use rstest::rstest;

    use super::{load_repo_data_recursively, PackageFilename, SparseRepoData};
    use crate::utils::test::fetch_repo_data;
    use fs_err as fs;

    fn test_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
    }

    async fn default_repo_data() -> Vec<(Channel, &'static str, PathBuf)> {
        tokio::try_join!(fetch_repo_data("linux-64"), fetch_repo_data("noarch")).unwrap();

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
        SparseRepoData::load_records_recursive(&sparse, package_names, None).unwrap()
    }

    async fn load_sparse(
        package_names: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Vec<Vec<RepoDataRecord>> {
        tokio::try_join!(fetch_repo_data("noarch"), fetch_repo_data("linux-64")).unwrap();

        //"linux-sha=20021d1dff9941ccf189f27404e296c54bc37fc4600c7027b366c03fc0bfa89e"
        //"noarch-sha=05e0c4ce7be29f36949c33cce782f21aecfbdd41f9e3423839670fb38fc5d691"

        load_repo_data_recursively(
            default_repo_data().await,
            package_names
                .into_iter()
                .map(|name| PackageName::try_from(name.as_ref()).unwrap()),
            None,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn test_empty_sparse_load() {
        let sparse_empty_data = load_sparse(Vec::<String>::new()).await;
        assert_eq!(sparse_empty_data, vec![vec![], vec![]]);
    }

    #[tokio::test]
    async fn test_sparse_single() {
        let sparse_empty_data = load_sparse(["_libgcc_mutex"]).await;
        let total_records = sparse_empty_data
            .iter()
            .map(std::vec::Vec::len)
            .sum::<usize>();

        assert_eq!(total_records, 3);
    }

    #[tokio::test]
    async fn test_parse_duplicate() {
        let sparse_empty_data = load_sparse(["_libgcc_mutex", "_libgcc_mutex"]).await;
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
        let sparse_empty_data = load_sparse(["jupyterlab", "detectron2"]).await;

        let total_records = sparse_empty_data
            .iter()
            .map(std::vec::Vec::len)
            .sum::<usize>();

        assert_eq!(total_records, 21732);
    }

    #[tokio::test]
    async fn test_sparse_rubin_env() {
        let sparse_empty_data = load_sparse(["rubin-env"]).await;

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
        let sparse_empty_data = load_sparse(package_names.clone()).await;

        let total_records = sparse_empty_data.iter().map(Vec::len).sum::<usize>();

        assert_eq!(total_records, 16065);

        // Bytes
        let repo_data = default_repo_data_bytes().await;
        let sparse_empty_data = load_sparse_from_bytes(&repo_data, package_names);

        let total_records = sparse_empty_data.iter().map(Vec::len).sum::<usize>();

        assert_eq!(total_records, 16065);
    }

    #[tokio::test]
    async fn load_complete_records() {
        tokio::try_join!(fetch_repo_data("noarch"), fetch_repo_data("linux-64")).unwrap();

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
        assert_eq!(sparse_repodata.package_names().try_len().unwrap(), 0);
    }
}
