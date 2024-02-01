//! This module provides the [`SparseRunExports`] which is a struct to enable only sparsely loading exports json
//! from a `run_exports.json` file.

use std::{io, path::Path};

use super::{PackageFilename, SparseData};
use futures::{stream, StreamExt, TryFutureExt, TryStreamExt};
use rattler_conda_types::{package::RunExportsJson, Channel, PackageName};
use serde_json::value::RawValue;
use superslice::Ext;

/// docs
pub type SparseRunExports = SparseData<RunExportsJson>;

impl SparseData<RunExportsJson> {
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

    /// Given a set of [`SparseRepoData`]s load all the records for the packages with the specified
    /// names and all the packages these records depend on.
    ///
    /// This will parse the records for the specified packages as well as all the packages these records
    /// depend on.
    ///
    pub fn load_run_exports_recursively<'a>(
        run_exports: impl IntoIterator<Item = &'a SparseData<RunExportsJson>>,
        package_names: impl IntoIterator<Item = PackageName>,
        patch_function: Option<fn(&mut RunExportsJson)>,
    ) -> io::Result<Vec<Vec<RunExportsJson>>> {
        let run_exports: Vec<&SparseData<RunExportsJson>> = run_exports.into_iter().collect();

        // Construct the result map
        let mut result = vec![vec![]; run_exports.len()];

        let packages: Vec<PackageName> = package_names.into_iter().collect();

        // Iterate over the list of packages that still need to be processed.
        for package_name in packages {
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
        #[derive(serde::Deserialize)]
        struct RunExportsWrapper {
            run_exports: RunExportsJson,
        }
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
