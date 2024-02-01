//! Sparse data parsing for conda package information such as, `repodata.json`, and `run_exports.json` files.

#![allow(clippy::mem_forget)]

use itertools::Itertools;
use rattler_conda_types::{Channel, ChannelInfo, PackageRecord};
use serde::{
    de::{Error, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::value::RawValue;
use std::{fmt, io, marker::PhantomData, path::Path};

pub mod repodata;
pub mod run_exports;

pub use repodata::*;
pub use run_exports::*;

/// Generalized struct for storing and loading data from large files using sparse data parsing.
pub struct SparseData<T: Sized> {
    /// Data structure that holds a memory mapped repodata.json file and an index into the the records
    /// store in that data.
    inner: SparseDataInner,

    /// The channel from which this data was downloaded.
    channel: Channel,

    /// The subdirectory from where the repodata is downloaded
    subdir: String,

    /// A function that can be used to patch the package record after it has been parsed.
    /// This is mainly used to add `pip` to `python` if desired
    patch_function: Option<fn(&mut T)>,
}

/// A struct that holds a memory map of a `repodata.json` file and also a self-referential field which
/// indexes the data in the memory map with a sparsely parsed json struct. See [`LazyRepoData`].

#[ouroboros::self_referencing]
struct SparseDataInner {
    /// Memory map of the `repodata.json` file
    memory_map: memmap2::Mmap,

    /// Sparsely parsed json content of the memory map. This data struct holds references into the memory
    /// map so we have to use ouroboros to make this legal.
    #[borrows(memory_map)]
    #[covariant]
    data: LazyMap<'this>,
}

impl<T: Sized> SparseData<T> {
    /// Construct an instance of self from a file on disk and a [`Channel`].
    /// The `patch_function` can be used to patch the package record after it has been parsed
    /// (e.g. to add `pip` to `python`).
    pub fn new(
        channel: Channel,
        subdir: impl Into<String>,
        path: impl AsRef<Path>,
        patch_function: Option<fn(&mut T)>,
    ) -> Result<Self, io::Error> {
        let file = std::fs::File::open(path)?;
        let memory_map = unsafe { memmap2::Mmap::map(&file) }?;
        Ok(SparseData {
            inner: SparseDataInnerTryBuilder {
                memory_map,
                data_builder: |memory_map| serde_json::from_slice(memory_map.as_ref()),
            }
            .try_build()?,
            subdir: subdir.into(),
            channel,
            patch_function,
        })
    }

    /// Returns an iterator over all package names in this repodata file.
    ///
    /// This works by iterating over all elements in the `packages` and `conda_packages` fields of
    /// the repodata and returning the unique package names.
    pub fn package_names(&self) -> impl Iterator<Item = &'_ str> + '_ {
        let repo_data = self.inner.borrow_data();
        repo_data
            .packages
            .iter()
            .chain(repo_data.conda_packages.iter())
            .map(|(name, _)| name.package)
            .dedup()
    }

    /// Returns the subdirectory from which this repodata was loaded
    pub fn subdir(&self) -> &str {
        &self.subdir
    }
}

/// A serde compatible struct that only sparsely parses a repodata.json file.
#[derive(Deserialize)]
struct LazyMap<'i> {
    /// The channel information contained in the repodata.json file
    info: Option<ChannelInfo>,

    /// The tar.bz2 packages contained in the data file
    #[serde(borrow, deserialize_with = "deserialize_filename_and_raw_record")]
    packages: Vec<(PackageFilename<'i>, &'i RawValue)>,

    /// The conda packages contained in the data file (under a different key for
    /// backwards compatibility with previous conda versions)
    #[serde(
        borrow,
        default,
        deserialize_with = "deserialize_filename_and_raw_record",
        rename = "packages.conda"
    )]
    conda_packages: Vec<(PackageFilename<'i>, &'i RawValue)>,
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

    // Although in general the filenames are sorted in repodata.json this doesnt necessarily mean
    // that the records are also sorted by package name.
    //
    // To illustrate, the following filenames are properly sorted by filename but they are NOT
    // properly sorted by package name.
    // - clang-format-12.0.1-default_he082bbe_4.tar.bz2 (package name: clang-format)
    // - clang-format-13-13.0.0-default_he082bbe_0.tar.bz2 (package name: clang-format-13)
    // - clang-format-13.0.0-default_he082bbe_0.tar.bz2 (package name: clang-format)
    //
    // Because most use-cases involve finding filenames by package name we reorder the entries here
    // by package name. This enables use the binary search for the packages we need.
    //
    // Since (in most cases) the repodata is already ordered by filename which does closely resemble
    // ordering by package name this sort operation will most likely be very fast.
    entries.sort_by(|(a, _), (b, _)| a.package.cmp(b.package));

    Ok(entries)
}

/// A struct that holds both a filename and the part of the filename thats just the package name.
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

impl<'de> TryFrom<&'de str> for PackageFilename<'de> {
    type Error = &'static str;

    fn try_from(s: &'de str) -> Result<Self, Self::Error> {
        let package = s.rsplitn(3, '-').nth(2).ok_or("invalid filename")?;
        Ok(PackageFilename {
            package,
            filename: s,
        })
    }
}

#[cfg(test)]
mod test {
    use super::{repodata::load_repo_data_recursively, PackageFilename};
    use rattler_conda_types::{Channel, ChannelConfig, PackageName, RepoData, RepoDataRecord};
    use rstest::rstest;
    use std::path::{Path, PathBuf};

    fn test_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
    }

    async fn load_sparse(
        package_names: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Vec<Vec<RepoDataRecord>> {
        load_repo_data_recursively(
            [
                (
                    Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap(),
                    "noarch",
                    test_dir().join("channels/conda-forge/noarch/repodata.json"),
                ),
                (
                    Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap(),
                    "linux-64",
                    test_dir().join("channels/conda-forge/linux-64/repodata.json"),
                ),
            ],
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

        // Number of records should still be 3. The duplicate package name should be ignored.
        assert_eq!(total_records, 3);
    }

    #[tokio::test]
    async fn test_sparse_jupyterlab_detectron2() {
        let sparse_empty_data = load_sparse(["jupyterlab", "detectron2"]).await;

        let total_records = sparse_empty_data
            .iter()
            .map(std::vec::Vec::len)
            .sum::<usize>();

        assert_eq!(total_records, 21731);
    }

    #[tokio::test]
    async fn test_sparse_numpy_dev() {
        let sparse_empty_data = load_sparse([
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
        ])
        .await;

        let total_records = sparse_empty_data
            .iter()
            .map(std::vec::Vec::len)
            .sum::<usize>();

        assert_eq!(total_records, 16064);
    }

    #[test]
    fn load_complete_records() {
        let mut records = Vec::new();
        for path in [
            test_dir().join("channels/conda-forge/noarch/repodata.json"),
            test_dir().join("channels/conda-forge/linux-64/repodata.json"),
        ] {
            let str = std::fs::read_to_string(&path).unwrap();
            let repo_data: RepoData = serde_json::from_str(&str).unwrap();
            records.push(repo_data);
        }

        let total_records = records
            .iter()
            .map(|repo| repo.conda_packages.len() + repo.packages.len())
            .sum::<usize>();

        assert_eq!(total_records, 367595);
    }

    #[rstest]
    #[case("clang-format-13.0.1-root_62800_h69bbbaa_1.conda", "clang-format")]
    #[case("clang-format-13-13.0.1-default_he082bbe_0.tar.bz2", "clang-format-13")]
    fn test_deserialize_package_name(#[case] filename: &str, #[case] result: &str) {
        assert_eq!(PackageFilename::try_from(filename).unwrap().package, result);
    }
}
