//! This module provides the [`SparseRepoData`] which is a struct to enable only sparsely loading records
//! from a `repodata.json` file.

use futures::{stream, StreamExt, TryFutureExt, TryStreamExt};
use rattler_conda_types::{Channel, PackageRecord, RepoDataRecord};
use serde::{
    de::{Error, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::value::RawValue;
use std::{
    collections::{HashSet, VecDeque},
    fmt, io,
    marker::PhantomData,
    path::Path,
};
use superslice::Ext;

/// A struct to enable loading records from a `repodata.json` file on demand. Since most of the time you
/// don't need all the records from the `repodata.json` this can help provide some significant speedups.
pub struct SparseRepoData {
    /// Data structure that holds a memory mapped repodata.json file and an index into the the records
    /// store in that data.
    inner: SparseRepoDataInner,

    /// The channel from which this data was downloaded.
    channel: Channel,
}

/// A struct that holds a memory map of a `repodata.json` file and also a self-referential field which
/// indexes the data in the memory map with a sparsely parsed json struct. See [`LazyRepoData`].
#[ouroboros::self_referencing]
struct SparseRepoDataInner {
    /// Memory map of the `repodata.json` file
    memory_map: memmap2::Mmap,

    /// Sparsely parsed json content of the memory map. This data struct holds references into the memory
    /// map so we have to use ouroboros to make this legal.
    #[borrows(memory_map)]
    #[covariant]
    repo_data: LazyRepoData<'this>,
}

impl SparseRepoData {
    /// Construct an instance of self from a file on disk and a [`Channel`].
    pub fn new(channel: Channel, path: impl AsRef<Path>) -> Result<Self, io::Error> {
        let file = std::fs::File::open(path)?;
        let memory_map = unsafe { memmap2::Mmap::map(&file) }?;
        Ok(SparseRepoData {
            inner: SparseRepoDataInnerTryBuilder {
                memory_map,

                repo_data_builder: |memory_map| serde_json::from_slice(memory_map.as_ref()),
            }
            .try_build()?,
            channel,
        })
    }

    /// Given a set of [`SparseRepoData`]s load all the records for the packages with the specified names.
    ///
    /// This will parse the records for the specified packages as well as all the packages these records
    /// depend on.
    pub fn load_records(
        repo_data: &[SparseRepoData],
        package_names: impl IntoIterator<Item = impl Into<String>>,
    ) -> io::Result<Vec<Vec<RepoDataRecord>>> {
        // Construct the result map
        let mut result = Vec::from_iter((0..repo_data.len()).map(|_| Vec::new()));

        // Construct a set of packages that we have seen and have been added to the pending list.
        let mut seen: HashSet<String> =
            HashSet::from_iter(package_names.into_iter().map(Into::into));

        // Construct a queue to store packages in that still need to be processed
        let mut pending = VecDeque::from_iter(seen.iter().cloned());

        // Iterate over the list of packages that still need to be processed.
        while let Some(next_package) = pending.pop_front() {
            for (i, repo_data) in repo_data.iter().enumerate() {
                let repo_data_packages = repo_data.inner.borrow_repo_data();

                // Get all records from the repodata
                let mut records = parse_records(
                    &next_package,
                    &repo_data_packages.packages,
                    &repo_data.channel,
                )?;
                let mut conda_records = parse_records(
                    &next_package,
                    &repo_data_packages.conda_packages,
                    &repo_data.channel,
                )?;
                records.append(&mut conda_records);

                // Iterate over all packages to find recursive dependencies.
                for record in records.iter() {
                    for dependency in &record.package_record.depends {
                        let dependency_name =
                            dependency.split_once(' ').unwrap_or((dependency, "")).0;
                        if !seen.contains(dependency_name) {
                            pending.push_back(dependency_name.to_string());
                            seen.insert(dependency_name.to_string());
                        }
                    }
                }

                result[i].append(&mut records);
            }
        }

        Ok(result)
    }
}

/// A serde compatible struct that only sparsely parses a repodata.json file.
#[derive(Deserialize)]
struct LazyRepoData<'i> {
    /// The tar.bz2 packages contained in the repodata.json file
    #[serde(borrow)]
    #[serde(deserialize_with = "deserialize_tuple_map")]
    packages: Vec<(PackageFilename<'i>, &'i RawValue)>,

    /// The conda packages contained in the repodata.json file (under a different key for
    /// backwards compatibility with previous conda versions)
    #[serde(borrow, rename = "packages.conda")]
    #[serde(deserialize_with = "deserialize_tuple_map")]
    conda_packages: Vec<(PackageFilename<'i>, &'i RawValue)>,
}

/// Parse the records for the specified package from the raw index
fn parse_records<'i>(
    package_name: &str,
    packages: &[(PackageFilename<'i>, &'i RawValue)],
    channel: &Channel,
) -> io::Result<Vec<RepoDataRecord>> {
    let channel_name = channel.canonical_name();

    let package_indices = packages.equal_range_by(|(package, _)| package.package.cmp(package_name));
    let mut result = Vec::with_capacity(package_indices.len());
    for (key, raw_json) in &packages[package_indices] {
        let package_record: PackageRecord = serde_json::from_str(raw_json.get())?;
        result.push(RepoDataRecord {
            url: channel
                .base_url()
                .join(&format!("{}/{}", &package_record.subdir, &key.filename))
                .expect("failed to build a url from channel and package record"),
            channel: channel_name.clone(),
            package_record,
            file_name: key.filename.to_owned(),
        });
    }
    Ok(result)
}

/// A helper function that immediately loads the records for the given packages (and their dependencies).
pub async fn load_repo_data_sparse(
    repo_data_paths: impl IntoIterator<Item = (Channel, impl AsRef<Path>)>,
    package_names: impl IntoIterator<Item = impl Into<String>>,
) -> Result<Vec<Vec<RepoDataRecord>>, io::Error> {
    // Open the different files and memory map them to get access to their bytes. Do this in parallel.
    let lazy_repo_data = stream::iter(repo_data_paths)
        .map(|(channel, path)| {
            let path = path.as_ref().to_path_buf();
            tokio::task::spawn_blocking(move || SparseRepoData::new(channel, path)).unwrap_or_else(
                |r| match r.try_into_panic() {
                    Ok(panic) => std::panic::resume_unwind(panic),
                    Err(err) => Err(io::Error::new(io::ErrorKind::Other, err.to_string())),
                },
            )
        })
        .buffered(50)
        .try_collect::<Vec<_>>()
        .await?;

    SparseRepoData::load_records(&lazy_repo_data, package_names)
}

fn deserialize_tuple_map<'d, D: Deserializer<'d>, K: Deserialize<'d>, V: Deserialize<'d>>(
    deserializer: D,
) -> Result<Vec<(K, V)>, D::Error> {
    return deserializer.deserialize_map(MapVisitor(PhantomData));

    #[allow(clippy::type_complexity)]
    struct MapVisitor<I, K, V>(PhantomData<fn() -> (I, K, V)>);

    impl<'de, I, K, V> Visitor<'de> for MapVisitor<I, K, V>
    where
        I: FromIterator<(K, V)>,
        K: Deserialize<'de>,
        V: Deserialize<'de>,
    {
        type Value = I;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
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
        let filename = <&str>::deserialize(deserializer)?;
        let package = filename
            .rsplitn(3, '-')
            .nth(2)
            .ok_or_else(|| D::Error::custom("invalid filename"))?;
        Ok(Self { package, filename })
    }
}

#[cfg(test)]
mod test {
    use crate::sparse::load_repo_data_sparse;
    use rattler_conda_types::{Channel, ChannelConfig, RepoData, RepoDataRecord};
    use std::path::{Path, PathBuf};

    fn test_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
    }

    async fn load_sparse(
        package_names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Vec<Vec<RepoDataRecord>> {
        load_repo_data_sparse(
            [
                (
                    Channel::from_str("conda-forge[linux-64]", &ChannelConfig::default()).unwrap(),
                    test_dir().join("channels/conda-forge/noarch/repodata.json"),
                ),
                (
                    Channel::from_str("conda-forge[linux-64]", &ChannelConfig::default()).unwrap(),
                    test_dir().join("channels/conda-forge/linux-64/repodata.json"),
                ),
            ],
            package_names,
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
            .map(|repo| repo.len())
            .sum::<usize>();

        assert_eq!(total_records, 3);
    }

    #[tokio::test]
    async fn test_parse_duplicate() {
        let sparse_empty_data = load_sparse(["_libgcc_mutex", "_libgcc_mutex"]).await;
        let total_records = sparse_empty_data
            .iter()
            .map(|repo| repo.len())
            .sum::<usize>();

        // Number of records should still be 3. The duplicate package name should be ignored.
        assert_eq!(total_records, 3);
    }

    #[tokio::test]
    async fn test_sparse_jupyterlab_detectron2() {
        let sparse_empty_data = load_sparse(["jupyterlab", "detectron2"]).await;

        let total_records = sparse_empty_data
            .iter()
            .map(|repo| repo.len())
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
            .map(|repo| repo.len())
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
}
