//! This module provides the [`SparseRepoData`] which is a struct to enable only sparsely loading records
//! from a `repodata.json` file.

use std::{
    collections::{HashSet, VecDeque},
    io,
    path::Path,
};

use super::{PackageFilename, PackageRecord, SparseData};
use futures::{stream, StreamExt, TryFutureExt, TryStreamExt};
use rattler_conda_types::{compute_package_url, Channel, PackageName, RepoDataRecord};
use serde_json::value::RawValue;
use superslice::Ext;

/// A struct to enable loading records from a `repodata.json` file on demand. Since most of the time you
/// don't need all the records from the `repodata.json` this can help provide some significant speedups.
pub type SparseRepoData = SparseData<PackageRecord>;

impl SparseData<PackageRecord> {
    /// Returns all the records for the specified package name.
    pub fn load_records(&self, package_name: &PackageName) -> io::Result<Vec<RepoDataRecord>> {
        let repo_data = self.inner.borrow_data();
        let base_url = repo_data.info.as_ref().and_then(|i| i.base_url.as_deref());
        let mut records = parse_records(
            package_name,
            &repo_data.packages,
            base_url,
            &self.channel,
            &self.subdir,
            self.patch_function,
        )?;
        let mut conda_records = parse_records(
            package_name,
            &repo_data.conda_packages,
            base_url,
            &self.channel,
            &self.subdir,
            self.patch_function,
        )?;
        records.append(&mut conda_records);
        Ok(records)
    }

    /// Given a set of [`SparseRepoData`]s load all the records for the packages with the specified
    /// names and all the packages these records depend on.
    ///
    /// This will parse the records for the specified packages as well as all the packages these records
    /// depend on.
    ///
    pub fn load_records_recursive<'a>(
        repo_data: impl IntoIterator<Item = &'a SparseData<PackageRecord>>,
        package_names: impl IntoIterator<Item = PackageName>,
        patch_function: Option<fn(&mut PackageRecord)>,
    ) -> io::Result<Vec<Vec<RepoDataRecord>>> {
        let repo_data: Vec<_> = repo_data.into_iter().collect();

        // Construct the result map
        let mut result = vec![vec![]; repo_data.len()];

        // Construct a set of packages that we have seen and have been added to the pending list.
        let mut seen: HashSet<PackageName> = package_names.into_iter().collect();

        // Construct a queue to store packages in that still need to be processed
        let mut pending: VecDeque<_> = seen.iter().cloned().collect();

        // Iterate over the list of packages that still need to be processed.
        while let Some(package_name) = pending.pop_front() {
            for (i, repo_data) in repo_data.iter().enumerate() {
                let repo_data_packages = repo_data.inner.borrow_data();
                let base_url = repo_data_packages
                    .info
                    .as_ref()
                    .and_then(|i| i.base_url.as_deref());

                // Get all records from the repodata
                let mut records = parse_records(
                    &package_name,
                    &repo_data_packages.packages,
                    base_url,
                    &repo_data.channel,
                    &repo_data.subdir,
                    patch_function,
                )?;
                let mut conda_records = parse_records(
                    &package_name,
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

    // note: this works as packages list is sorted by package name
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

/// A helper function that immediately loads the records for the given packages (and their dependencies).
/// Records for the specified packages are loaded from the repodata files.
/// The `patch_record_fn` is applied to each record after it has been parsed and can mutate the record after
/// it has been loaded.
pub async fn load_repo_data_recursively(
    repo_data_paths: impl IntoIterator<Item = (Channel, impl Into<String>, impl AsRef<Path>)>,
    package_names: impl IntoIterator<Item = PackageName>,
    patch_function: Option<fn(&mut PackageRecord)>,
) -> Result<Vec<Vec<RepoDataRecord>>, io::Error> {
    // Open the different files and memory map them to get access to their bytes. Do this in parallel.
    let lazy_repo_data = stream::iter(repo_data_paths)
        .map(|(channel, subdir, path)| {
            let path = path.as_ref().to_path_buf();
            let subdir = subdir.into();
            tokio::task::spawn_blocking(move || {
                SparseData::new(channel, subdir, path, patch_function)
            })
            .unwrap_or_else(|r| match r.try_into_panic() {
                Ok(panic) => std::panic::resume_unwind(panic),
                Err(err) => Err(io::Error::new(io::ErrorKind::Other, err.to_string())),
            })
        })
        .buffered(50)
        .try_collect::<Vec<_>>()
        .await?;

    SparseData::load_records_recursive(&lazy_repo_data, package_names, patch_function)
}

#[cfg(test)]
mod test {
    use super::super::{repodata::load_repo_data_recursively, PackageFilename};
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
        dbg!(&sparse_empty_data);
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
