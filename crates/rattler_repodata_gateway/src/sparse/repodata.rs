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

    SparseRepoData::load_records_recursive(&lazy_repo_data, package_names, patch_function)
}
