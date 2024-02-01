//! This module provides the [`SparseRunExports`] which is a struct to enable only sparsely loading exports json
//! from a `run_exports.json` file.

use std::{collections::HashSet, io, path::Path};

use super::{PackageFilename, SparseData};
use futures::{stream, StreamExt, TryFutureExt, TryStreamExt};
use rattler_conda_types::{package::RunExportsJson, Channel, PackageName};
use serde_json::value::RawValue;
use superslice::Ext;

/// Provides on-demand loading of [`RunExportsJson`] from a `run_exports.json` file.
/// Since all exports for very few packages are required at once, this helps provide significant speedups.
pub type SparseRunExports = SparseData<RunExportsJson>;

impl SparseRunExports {
    /// Returns all the records for the specified package name.
    pub fn load_records(&self, package_name: &PackageName) -> io::Result<Vec<RunExportsJson>> {
        let exports_map = self.inner.borrow_data();
        let mut exports =
            parse_run_exports(package_name, &exports_map.packages, self.patch_function)?;
        let mut conda_exports = parse_run_exports(
            package_name,
            &exports_map.conda_packages,
            self.patch_function,
        )?;
        exports.append(&mut conda_exports);
        Ok(exports)
    }

    /// Given a set of [`SparseRunExports`]s load all the records for the packages with the specified
    /// names and all the packages these records depend on.
    ///
    /// This will parse the records for the specified packages as well as all the packages these records
    /// depend on.
    ///
    pub fn load_run_exports_recursively<'a>(
        run_exports: impl IntoIterator<Item = &'a SparseRunExports>,
        package_names: impl IntoIterator<Item = PackageName>,
        patch_function: Option<fn(&mut RunExportsJson)>,
    ) -> io::Result<Vec<Vec<RunExportsJson>>> {
        let package_names: HashSet<PackageName> = package_names.into_iter().collect();
        let run_exports: Vec<&SparseRunExports> = run_exports.into_iter().collect();

        // Construct the result map
        let mut result = vec![vec![]; run_exports.len()];

        // Iterate over the list of packages that still need to be processed.
        for package_name in package_names {
            for (i, run_export) in run_exports.iter().enumerate() {
                let run_export = run_export as &SparseData<RunExportsJson>;
                let run_export_packages = run_export.inner.borrow_data();
                let mut exports = parse_run_exports(
                    &package_name,
                    &run_export_packages.packages,
                    patch_function,
                )?;
                let mut conda_run_exports = parse_run_exports(
                    &package_name,
                    &run_export_packages.conda_packages,
                    patch_function,
                )?;
                exports.append(&mut conda_run_exports);
                result[i].append(&mut exports);
            }
        }

        Ok(result)
    }
}

#[derive(serde::Deserialize)]
struct RunExportsWrapper {
    run_exports: RunExportsJson,
}

/// Parse the records for the specified package from the raw index
fn parse_run_exports<'i>(
    package_name: &PackageName,
    packages: &[(PackageFilename<'i>, &'i RawValue)],
    patch_function: Option<fn(&mut RunExportsJson)>,
) -> io::Result<Vec<RunExportsJson>> {
    let package_indices =
        packages.equal_range_by(|(package, _)| package.package.cmp(package_name.as_normalized()));
    let mut result = Vec::with_capacity(package_indices.len());
    for (_, raw_json) in &packages[package_indices] {
        // run exports are stored inside the run_exports field
        let rew: RunExportsWrapper = serde_json::from_str(raw_json.get())?;
        result.push(rew.run_exports);
    }

    // Apply the patch function if one was specified
    if let Some(patch_fn) = patch_function {
        for record in &mut result {
            patch_fn(record);
        }
    }

    Ok(result)
}

/// A helper function that immediately loads the records for the given packages (and their dependencies).
/// Records for the specified packages are loaded from the run_exports files.
/// The `patch_record_fn` is applied to each record after it has been parsed and can mutate the record after
/// it has been loaded.
pub async fn load_run_exports_recursively(
    run_export_paths: impl IntoIterator<Item = (Channel, impl Into<String>, impl AsRef<Path>)>,
    package_names: impl IntoIterator<Item = PackageName>,
    patch_function: Option<fn(&mut RunExportsJson)>,
) -> Result<Vec<Vec<RunExportsJson>>, io::Error> {
    // Open the different files and memory map them to get access to their bytes. Do this in parallel.
    let lazy_run_exports = stream::iter(run_export_paths)
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

    SparseRunExports::load_run_exports_recursively(&lazy_run_exports, package_names, patch_function)
}

#[cfg(test)]
mod test {
    use super::super::{run_exports::load_run_exports_recursively, PackageFilename};
    use crate::sparse::run_exports::RunExportsWrapper;
    use rattler_conda_types::{package::RunExportsJson, Channel, ChannelConfig, PackageName};
    use rstest::rstest;
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
    };

    #[derive(serde::Deserialize)]
    struct RunExportsData {
        #[serde(
            default,
            rename = "packages.conda",
            serialize_with = "sort_map_alphabetically"
        )]
        packages_conda: HashMap<String, RunExportsWrapper>,
        #[serde(serialize_with = "sort_map_alphabetically")]
        packages: HashMap<String, RunExportsWrapper>,
    }
    fn test_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
    }

    async fn load_sparse(
        package_names: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Vec<Vec<RunExportsJson>> {
        load_run_exports_recursively(
            [
                (
                    Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap(),
                    "noarch",
                    test_dir().join("channels/conda-forge/noarch/run_exports.json"),
                ),
                (
                    Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap(),
                    "linux-64",
                    test_dir().join("channels/conda-forge/linux-64/run_exports.json"),
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

        assert_eq!(total_records, 799);
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

        assert_eq!(total_records, 5017);
    }

    #[test]
    fn load_complete_records() {
        let mut exports = Vec::new();
        for path in [
            test_dir().join("channels/conda-forge/noarch/run_exports.json"),
            test_dir().join("channels/conda-forge/linux-64/run_exports.json"),
        ] {
            let src = std::fs::read_to_string(&path).unwrap();
            let repo_data: RunExportsData = serde_json::from_str(&src).unwrap();
            exports.push(repo_data);
        }

        let total_records = exports
            .iter()
            .map(|repo| repo.packages_conda.len() + repo.packages.len())
            .sum::<usize>();

        assert_eq!(total_records, 615990);
    }

    #[rstest]
    #[case("clang-format-13.0.1-root_62800_h69bbbaa_1.conda", "clang-format")]
    #[case("clang-format-13-13.0.1-default_he082bbe_0.tar.bz2", "clang-format-13")]
    fn test_deserialize_package_name(#[case] filename: &str, #[case] result: &str) {
        assert_eq!(PackageFilename::try_from(filename).unwrap().package, result);
    }
}
