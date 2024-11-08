//! Implements a registry for "clobbering" files (files that are appearing in
//! multiple packages)

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use fs_err as fs;
use indexmap::IndexSet;
use itertools::Itertools;
use rattler_conda_types::{
    package::{IndexJson, PathsEntry},
    PackageName, PrefixRecord,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClobberedPath {
    /// The name of the package from which the final file is taken.
    pub package: PackageName,

    /// Other packages that clobbered the file.
    pub other_packages: Vec<PackageName>,
}

#[derive(Debug, thiserror::Error)]
pub enum ClobberError {
    #[error("{0}")]
    IoError(String, #[source] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathState {
    /// A path that is installed after the transaction by a package
    Installed(PackageNameIdx),
    /// A path that is removed after the transaction by a package
    Removed(PackageNameIdx),
}

/// A registry for clobbering files
/// The registry keeps track of all files that are installed by a package and
/// can be used to rename files that are already installed by another package.
#[derive(Debug, Default, Clone)]
pub struct ClobberRegistry {
    /// A cache of package names
    package_names: Vec<PackageName>,

    /// The paths that exist in the prefix and the first package that touched
    /// the file.
    paths_registry: HashMap<PathBuf, PathState>,

    /// Paths that have been clobbered and by which package, this also
    /// includes the primary package. E.g. the package that actually wrote to
    /// the file.
    clobbers: HashMap<PathBuf, Vec<PackageNameIdx>>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
struct PackageNameIdx(usize);

static CLOBBER_TEMPLATE: &str = "__clobber-from-";

fn clobber_template(package_name: &PackageName) -> String {
    format!("{CLOBBER_TEMPLATE}{}", package_name.as_normalized())
}

impl ClobberRegistry {
    /// Create a new clobber registry that is initialized with the given prefix
    /// records.
    pub fn new<'i>(prefix_records: impl IntoIterator<Item = &'i PrefixRecord>) -> Self {
        let mut package_names = Vec::new();
        let mut paths_registry = HashMap::new();
        let mut temp_clobbers = Vec::new();

        for prefix_record in prefix_records {
            let package_name = prefix_record.repodata_record.package_record.name.clone();
            package_names.push(package_name.clone());
            let package_name_idx = PackageNameIdx(package_names.len() - 1);

            for p in &prefix_record.paths_data.paths {
                if let Some(original_path) = &p.original_path {
                    temp_clobbers.push((original_path, package_name_idx));
                } else {
                    paths_registry.insert(
                        p.relative_path.clone(),
                        PathState::Installed(package_name_idx),
                    );
                }
            }
        }

        let mut clobbers = HashMap::with_capacity(temp_clobbers.len());
        for (path, originating_package_idx) in temp_clobbers.iter() {
            let path = *path;
            clobbers
                .entry(path.clone())
                .or_insert_with(|| {
                    // The path can only be installed at this point
                    if let Some(&PathState::Installed(other_idx)) = paths_registry.get(path) {
                        vec![other_idx]
                    } else {
                        Vec::new()
                    }
                })
                .push(*originating_package_idx);
        }

        Self {
            package_names,
            paths_registry,
            clobbers,
        }
    }

    /// Register that all the paths of a package are being removed.
    pub fn unregister_paths(&mut self, prefix_paths: &PrefixRecord) {
        // Find the name in the registry
        let Some(name_idx) = self
            .package_names
            .iter()
            .position(|n| n == &prefix_paths.repodata_record.package_record.name)
            .map(PackageNameIdx)
        else {
            tracing::warn!(
                "Tried to unregister paths for a package ({}) that is not in the registry",
                prefix_paths
                    .repodata_record
                    .package_record
                    .name
                    .as_normalized()
            );
            return;
        };

        // Remove this package from any clobbering consideration.
        for p in &prefix_paths.paths_data.paths {
            let path = p.original_path.as_ref().unwrap_or(&p.relative_path);
            if let Some(clobber) = self.clobbers.get_mut(path) {
                clobber.retain(|&idx| idx != name_idx);
            }

            let Some(paths_entry) = self.paths_registry.get_mut(path) else {
                tracing::warn!("The path {} is not in the registry", path.display());
                continue;
            };

            if *paths_entry == PathState::Installed(name_idx) {
                *paths_entry = PathState::Removed(name_idx);
            }
        }
    }

    /// Register the paths of a package before linking a package in
    /// order to determine which files may clobber other files (clobbering files
    /// are those that are present in multiple packages).
    ///
    /// This function has to run sequentially, and a `post_process` step
    /// will "unclobber" the files after all packages have been installed.
    pub fn register_paths(
        &mut self,
        index_json: &IndexJson,
        computed_paths: &Vec<(PathsEntry, PathBuf)>,
    ) -> HashMap<PathBuf, PathBuf> {
        let mut clobber_paths = HashMap::new();
        let name = &index_json.name.clone();

        // check if we have the package name already registered
        let name_idx = if let Some(idx) = self.package_names.iter().position(|n| n == name) {
            PackageNameIdx(idx)
        } else {
            self.package_names.push(name.clone());
            PackageNameIdx(self.package_names.len() - 1)
        };

        for (_, path) in computed_paths {
            if let Some(&entry) = self.paths_registry.get(path) {
                match entry {
                    PathState::Installed(idx) => {
                        // if we find an entry, we have a clobbering path!
                        // Then we rename the current path to a clobbered path
                        let new_path = clobber_name(path, &self.package_names[name_idx.0]);
                        self.clobbers
                            .entry(path.clone())
                            .or_insert_with(|| vec![idx])
                            .push(name_idx);

                        // We insert the non-renamed path here
                        clobber_paths.insert(path.clone(), new_path);
                    }
                    PathState::Removed(idx) => {
                        if idx == name_idx {
                            // This is just an update of the package itself so we don't need to
                            // do anything special (just flip it as installed)
                            self.paths_registry
                                .insert(path.clone(), PathState::Installed(idx));
                            // If we previously had clobbers with this path, we need to
                            // add the re-installed package back to the clobbers
                            if let Some(entry) = self.clobbers.get_mut(path) {
                                entry.push(name_idx);
                            }
                        } else {
                            // In this case, another package is installing this path. We have previously
                            // removed this path, but since we don't know about the order of execution of
                            // removals and installs _on the disc_ we need to first install this path to a clobbering
                            // path and then rename it back to the original path after everything has finished.
                            let new_path = clobber_name(path, &self.package_names[name_idx.0]);
                            self.clobbers
                                .entry(path.clone())
                                // We insert an empty vector here because there is no other file that should stick around
                                // (idx is already removed)
                                .or_default()
                                .push(name_idx);

                            // We insert the non-renamed path here
                            clobber_paths.insert(path.clone(), new_path);
                        }
                    }
                }
            } else {
                self.paths_registry
                    .insert(path.clone(), PathState::Installed(name_idx));
            }
        }

        clobber_paths
    }

    /// Unclobber the paths after all installation steps have been completed.
    /// Returns an overview of all the clobbered files.
    pub fn unclobber(
        &mut self,
        sorted_prefix_records: &[&PrefixRecord],
        target_prefix: &Path,
    ) -> Result<HashMap<PathBuf, ClobberedPath>, ClobberError> {
        let conda_meta = target_prefix.join("conda-meta");
        let sorted_names = sorted_prefix_records
            .iter()
            .map(|p| p.repodata_record.package_record.name.clone())
            .collect::<IndexSet<_>>();

        let mut prefix_records = sorted_prefix_records
            .iter()
            .map(|x| (*x).clone())
            .collect::<Vec<PrefixRecord>>();
        let mut prefix_records_to_rewrite = HashSet::new();
        let mut result = HashMap::new();

        tracing::info!("Unclobbering {} files", self.clobbers.len());
        for (path, clobbered_by) in self.clobbers.iter() {
            let clobbered_by_names = clobbered_by
                .iter()
                .map(|&idx| &self.package_names[idx.0])
                .collect::<IndexSet<_>>();

            // Extract the subset of clobbered_by that is in sorted_prefix_records
            let sorted_clobbered_by = sorted_names
                .iter()
                .cloned()
                .enumerate()
                .filter(|(_, n)| clobbered_by_names.contains(n))
                .collect::<Vec<_>>();

            let Some(current_winner_entry) = self.paths_registry.get(path) else {
                tracing::warn!(
                    "The path {} is clobbered but not in the registry",
                    path.display()
                );
                continue;
            };

            // let current_winner = current_winner_entry.map(|idx| &self.package_names[idx.0]);
            let current_winner = match current_winner_entry {
                PathState::Installed(idx) => Some(&self.package_names[idx.0]),
                PathState::Removed(_) => None,
            };

            // Determine which package should write to the file
            let winner = match sorted_clobbered_by.last() {
                Some(winner) => winner,
                // In this case, all files have been removed and we can skip any unclobbering
                None => continue,
            };

            if clobbered_by.len() > 1 {
                tracing::info!(
                    "The path {} is clobbered by multiple packages ({}) but ultimately the file from {} is kept.",
                    path.display(),
                    sorted_clobbered_by.iter().map(|(_, n)| n.as_normalized()).format(", "),
                    &winner.1.as_normalized()
                );
            }

            if clobbered_by.len() > 1 {
                result.insert(
                    path.clone(),
                    ClobberedPath {
                        package: winner.1.clone(),
                        other_packages: sorted_clobbered_by
                            .iter()
                            .rev()
                            .skip(1)
                            .rev()
                            .map(|(_, n)| n.clone())
                            .collect(),
                    },
                );
            }

            // If the package that wrote to the file initially is already the package that
            // should write it, we can skip modifying this file in the first place.
            if Some(&winner.1) == current_winner {
                continue;
            }

            // If the path currently exists, we need to rename it.
            let full_path = target_prefix.join(path);
            if full_path.exists() {
                if let Some(loser_name) = current_winner {
                    let loser_path = clobber_name(path, loser_name);

                    // Rename the original file to a clobbered path.
                    tracing::debug!("renaming {} to {}", path.display(), loser_path.display());
                    fs::rename(target_prefix.join(path), target_prefix.join(&loser_path)).map_err(
                        |e| {
                            ClobberError::IoError(
                                format!(
                                    "failed to rename {} to {}",
                                    path.display(),
                                    loser_path.display()
                                ),
                                e,
                            )
                        },
                    )?;

                    if let Some(loser_idx) = sorted_clobbered_by
                        .iter()
                        .find(|(_, n)| n == loser_name)
                        .map(|(idx, _)| *idx)
                    {
                        rename_path_in_prefix_record(
                            &mut prefix_records[loser_idx],
                            path,
                            &loser_path,
                            true,
                        );
                        prefix_records_to_rewrite.insert(loser_idx);
                    }
                }
            }

            // Rename the winner
            let winner_path = clobber_name(path, &winner.1);
            tracing::debug!("renaming {} to {}", winner_path.display(), path.display());
            fs::rename(target_prefix.join(&winner_path), target_prefix.join(path)).map_err(
                |e| {
                    ClobberError::IoError(
                        format!(
                            "failed to rename {} to {}",
                            winner_path.display(),
                            path.display()
                        ),
                        e,
                    )
                },
            )?;

            rename_path_in_prefix_record(&mut prefix_records[winner.0], &winner_path, path, false);

            prefix_records_to_rewrite.insert(winner.0);
        }

        for idx in prefix_records_to_rewrite {
            let rec = &prefix_records[idx];
            tracing::debug!(
                "writing updated prefix record to: {:?}",
                conda_meta.join(rec.file_name())
            );
            rec.write_to_path(conda_meta.join(rec.file_name()), true)
                .map_err(|e| {
                    ClobberError::IoError(
                        format!("failed to write updated prefix record {}", rec.file_name()),
                        e,
                    )
                })?;
        }

        Ok(result)
    }
}

fn clobber_name(path: &Path, package_name: &PackageName) -> PathBuf {
    let file_name = path.file_name().unwrap_or_default();
    let mut new_path = path.to_path_buf();
    new_path.set_file_name(format!(
        "{}{}",
        file_name.to_string_lossy(),
        clobber_template(package_name),
    ));
    new_path
}

fn rename_path_in_prefix_record(
    record: &mut PrefixRecord,
    old_path: &Path,
    new_path: &Path,
    new_path_is_clobber: bool,
) {
    for path in record.files.iter_mut() {
        if path == old_path {
            *path = new_path.to_path_buf();
        }
    }

    for path in record.paths_data.paths.iter_mut() {
        if path.relative_path == old_path {
            path.relative_path = new_path.to_path_buf();
            path.original_path = if new_path_is_clobber {
                Some(old_path.to_path_buf())
            } else {
                None
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        str::FromStr,
    };

    use insta::assert_yaml_snapshot;
    use rand::seq::SliceRandom;
    use rattler_conda_types::{Platform, PrefixRecord, RepoDataRecord, Version};
    use transaction::TransactionOperation;

    use crate::{
        get_repodata_record, get_test_data_dir,
        install::{test_utils::*, transaction, InstallDriver, InstallOptions, PythonInfo},
        package_cache::PackageCache,
    };

    fn test_operations() -> Vec<TransactionOperation<PrefixRecord, RepoDataRecord>> {
        let repodata_record_1 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-1-0.1.0-h4616a5c_0.tar.bz2"),
        );
        let repodata_record_2 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-2-0.1.0-h4616a5c_0.tar.bz2"),
        );
        let repodata_record_3 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-3-0.1.0-h4616a5c_0.tar.bz2"),
        );

        vec![
            TransactionOperation::Install(repodata_record_1),
            TransactionOperation::Install(repodata_record_2),
            TransactionOperation::Install(repodata_record_3),
        ]
    }

    fn test_python_noarch_operations() -> Vec<TransactionOperation<PrefixRecord, RepoDataRecord>> {
        let repodata_record_1 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-pynoarch-1-0.1.0-pyh4616a5c_0.tar.bz2"),
        );
        let repodata_record_2 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-pynoarch-2-0.1.0-pyh4616a5c_0.tar.bz2"),
        );

        vec![
            TransactionOperation::Install(repodata_record_1),
            TransactionOperation::Install(repodata_record_2),
        ]
    }

    fn test_operations_nested() -> Vec<TransactionOperation<PrefixRecord, RepoDataRecord>> {
        let repodata_record_1 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-nested-1-0.1.0-h4616a5c_0.tar.bz2"),
        );
        let repodata_record_2 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-nested-2-0.1.0-h4616a5c_0.tar.bz2"),
        );
        let repodata_record_3 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-nested-3-0.1.0-h4616a5c_0.tar.bz2"),
        );

        vec![
            TransactionOperation::Install(repodata_record_1),
            TransactionOperation::Install(repodata_record_2),
            TransactionOperation::Install(repodata_record_3),
        ]
    }

    fn test_operations_update() -> Vec<RepoDataRecord> {
        let repodata_record_1 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-1-0.2.0-h4616a5c_0.tar.bz2"),
        );
        let repodata_record_2 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-2-0.2.0-h4616a5c_0.tar.bz2"),
        );
        let repodata_record_3 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-3-0.2.0-h4616a5c_0.tar.bz2"),
        );

        vec![repodata_record_1, repodata_record_2, repodata_record_3]
    }

    fn assert_check_files(target_prefix: &Path, expected_files: &[&str]) {
        let files = std::fs::read_dir(target_prefix).unwrap();
        let files = files
            .filter_map(|f| {
                let fx = f.unwrap();
                if fx.file_type().unwrap().is_file() {
                    Some(fx.path().strip_prefix(target_prefix).unwrap().to_path_buf())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(files.len(), expected_files.len());
        for file in &files {
            assert!(
                expected_files.contains(&file.file_name().unwrap().to_string_lossy().as_ref()),
                "file {} is not expected. Expected:\n{expected_files:#?}\n\nFound:\n{files:#?}",
                file.file_name().unwrap().to_string_lossy()
            );
        }
    }

    #[tokio::test]
    async fn test_transaction_with_clobber() {
        // Create a transaction
        let operations = test_operations();

        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations,
            python_info: None,
            current_python_info: None,
            platform: Platform::current(),
        };

        // execute transaction
        let target_prefix = tempfile::tempdir().unwrap();

        let packages_dir = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        execute_transaction(
            transaction,
            target_prefix.path(),
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &InstallDriver::default(),
            &InstallOptions::default(),
        )
        .await;

        // check that the files are there
        assert_check_files(
            target_prefix.path(),
            &[
                "clobber.txt",
                "clobber.txt__clobber-from-clobber-2",
                "clobber.txt__clobber-from-clobber-3",
                "another-clobber.txt",
                "another-clobber.txt__clobber-from-clobber-2",
                "another-clobber.txt__clobber-from-clobber-3",
            ],
        );

        let prefix_records = PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();

        let prefix_record_clobber_1 = find_prefix_record(&prefix_records, "clobber-1").unwrap();
        assert_yaml_snapshot!(prefix_record_clobber_1.files);
        assert_yaml_snapshot!(prefix_record_clobber_1.paths_data);
        let prefix_record_clobber_2 = find_prefix_record(&prefix_records, "clobber-2").unwrap();
        assert_yaml_snapshot!(prefix_record_clobber_2.files);
        assert_yaml_snapshot!(prefix_record_clobber_2.paths_data);

        assert_eq!(
            fs::read_to_string(target_prefix.path().join("clobber.txt")).unwrap(),
            "clobber-1\n"
        );

        // remove one of the clobbering files
        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations: vec![TransactionOperation::Remove(
                prefix_record_clobber_1.clone(),
            )],
            python_info: None,
            current_python_info: None,
            platform: Platform::current(),
        };

        let install_driver = InstallDriver::builder()
            .with_prefix_records(&prefix_records)
            .execute_link_scripts(true)
            .finish();

        execute_transaction(
            transaction,
            target_prefix.path(),
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &install_driver,
            &InstallOptions::default(),
        )
        .await;

        assert_check_files(
            target_prefix.path(),
            &[
                "clobber.txt__clobber-from-clobber-3",
                "clobber.txt",
                "another-clobber.txt__clobber-from-clobber-3",
                "another-clobber.txt",
            ],
        );

        let prefix_records = PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();

        let prefix_record_clobber_1 = find_prefix_record(&prefix_records, "clobber-1");
        assert!(prefix_record_clobber_1.is_none());
        let prefix_record_clobber_2 = find_prefix_record(&prefix_records, "clobber-2").unwrap();
        assert_yaml_snapshot!(prefix_record_clobber_2.files);
        assert_yaml_snapshot!(prefix_record_clobber_2.paths_data);

        assert_eq!(
            fs::read_to_string(target_prefix.path().join("clobber.txt")).unwrap(),
            "clobber-2\n"
        );
        assert_eq!(
            fs::read_to_string(target_prefix.path().join("another-clobber.txt")).unwrap(),
            "clobber-2\n"
        );
        let prefix_record_clobber_3 = find_prefix_record(&prefix_records, "clobber-3").unwrap();
        assert!(
            prefix_record_clobber_3.files
                == vec![
                    PathBuf::from("another-clobber.txt__clobber-from-clobber-3"),
                    PathBuf::from("clobber.txt__clobber-from-clobber-3")
                ]
        );

        assert_eq!(prefix_record_clobber_3.paths_data.paths.len(), 2);
        assert_yaml_snapshot!(prefix_record_clobber_3.paths_data);
    }

    #[tokio::test]
    async fn test_random_clobber() {
        for _ in 0..3 {
            let mut operations = test_operations();
            // randomize the order of the operations
            operations.shuffle(&mut rand::thread_rng());

            let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
                operations,
                python_info: None,
                current_python_info: None,
                platform: Platform::current(),
            };

            // execute transaction
            let target_prefix = tempfile::tempdir().unwrap();

            let packages_dir = tempfile::tempdir().unwrap();
            let cache = PackageCache::new(packages_dir.path());

            execute_transaction(
                transaction,
                target_prefix.path(),
                &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
                &cache,
                &InstallDriver::default(),
                &InstallOptions::default(),
            )
            .await;

            assert_eq!(
                fs::read_to_string(target_prefix.path().join("clobber.txt")).unwrap(),
                "clobber-1\n"
            );

            // make sure that clobbers are resolved deterministically
            assert_check_files(
                target_prefix.path(),
                &[
                    "clobber.txt__clobber-from-clobber-3",
                    "clobber.txt__clobber-from-clobber-2",
                    "clobber.txt",
                    "another-clobber.txt__clobber-from-clobber-3",
                    "another-clobber.txt__clobber-from-clobber-2",
                    "another-clobber.txt",
                ],
            );

            let prefix_records = PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();

            for record in prefix_records {
                if record.repodata_record.package_record.name.as_normalized() == "clobber-1" {
                    assert_eq!(
                        record.files,
                        vec![
                            PathBuf::from("another-clobber.txt"),
                            PathBuf::from("clobber.txt")
                        ]
                    );
                } else if record.repodata_record.package_record.name.as_normalized() == "clobber-2"
                {
                    assert_eq!(
                        record.files,
                        vec![
                            PathBuf::from("another-clobber.txt__clobber-from-clobber-2"),
                            PathBuf::from("clobber.txt__clobber-from-clobber-2")
                        ]
                    );
                } else if record.repodata_record.package_record.name.as_normalized() == "clobber-3"
                {
                    assert_eq!(
                        record.files,
                        vec![
                            PathBuf::from("another-clobber.txt__clobber-from-clobber-3"),
                            PathBuf::from("clobber.txt__clobber-from-clobber-3")
                        ]
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn test_random_clobber_nested() {
        for _ in 0..3 {
            let mut operations = test_operations_nested();
            // randomize the order of the operations
            operations.shuffle(&mut rand::thread_rng());

            let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
                operations,
                python_info: None,
                current_python_info: None,
                platform: Platform::current(),
            };

            // execute transaction
            let target_prefix = tempfile::tempdir().unwrap();

            let packages_dir = tempfile::tempdir().unwrap();
            let cache = PackageCache::new(packages_dir.path());

            execute_transaction(
                transaction,
                target_prefix.path(),
                &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
                &cache,
                &InstallDriver::default(),
                &InstallOptions::default(),
            )
            .await;

            assert_eq!(
                fs::read_to_string(target_prefix.path().join("clobber/bobber/clobber.txt"))
                    .unwrap(),
                "clobber-2\n"
            );

            // make sure that clobbers are resolved deterministically
            assert_check_files(
                &target_prefix.path().join("clobber/bobber"),
                &[
                    "clobber.txt__clobber-from-clobber-nested-3",
                    "clobber.txt__clobber-from-clobber-nested-1",
                    "clobber.txt",
                ],
            );

            let prefix_records = PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();
            let prefix_record_clobber_2 =
                find_prefix_record(&prefix_records, "clobber-nested-2").unwrap();
            let prefix_record_clobber_3 =
                find_prefix_record(&prefix_records, "clobber-nested-3").unwrap();

            assert_eq!(
                prefix_record_clobber_3.files,
                vec![PathBuf::from(
                    "clobber/bobber/clobber.txt__clobber-from-clobber-nested-3"
                )]
            );

            // remove one of the clobbering files
            let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
                operations: vec![TransactionOperation::Remove(
                    prefix_record_clobber_2.clone(),
                )],
                python_info: None,
                current_python_info: None,
                platform: Platform::current(),
            };

            let install_driver = InstallDriver::builder()
                .with_prefix_records(&prefix_records)
                .finish();

            execute_transaction(
                transaction,
                target_prefix.path(),
                &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
                &cache,
                &install_driver,
                &InstallOptions::default(),
            )
            .await;

            assert_check_files(
                &target_prefix.path().join("clobber/bobber"),
                &["clobber.txt__clobber-from-clobber-nested-3", "clobber.txt"],
            );

            assert_eq!(
                fs::read_to_string(target_prefix.path().join("clobber/bobber/clobber.txt"))
                    .unwrap(),
                "clobber-1\n"
            );

            let prefix_records = PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();
            let prefix_record_clobber_1 =
                find_prefix_record(&prefix_records, "clobber-nested-1").unwrap();

            assert_eq!(
                prefix_record_clobber_1.files,
                vec![PathBuf::from("clobber/bobber/clobber.txt")]
            );
        }
    }

    #[tokio::test]
    async fn test_clobber_update() {
        // Create a transaction
        let operations = test_operations();

        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations,
            python_info: None,
            current_python_info: None,
            platform: Platform::current(),
        };

        // execute transaction
        let target_prefix = tempfile::tempdir().unwrap();

        let packages_dir = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        execute_transaction(
            transaction,
            target_prefix.path(),
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &InstallDriver::default(),
            &InstallOptions::default(),
        )
        .await;

        // check that the files are there
        assert_check_files(
            target_prefix.path(),
            &[
                "clobber.txt",
                "clobber.txt__clobber-from-clobber-2",
                "clobber.txt__clobber-from-clobber-3",
                "another-clobber.txt",
                "another-clobber.txt__clobber-from-clobber-2",
                "another-clobber.txt__clobber-from-clobber-3",
            ],
        );

        println!("== RUNNING UPDATE");

        let mut prefix_records = PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();
        prefix_records.sort_by(|a, b| {
            a.repodata_record
                .package_record
                .name
                .as_normalized()
                .cmp(b.repodata_record.package_record.name.as_normalized())
        });

        let update_ops = test_operations_update();

        // remove one of the clobbering files
        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations: vec![TransactionOperation::Change {
                old: prefix_records[0].clone(),
                new: update_ops[0].clone(),
            }],
            python_info: None,
            current_python_info: None,
            platform: Platform::current(),
        };

        let install_driver = InstallDriver::builder()
            .with_prefix_records(&prefix_records)
            .finish();

        let result = execute_transaction(
            transaction,
            target_prefix.path(),
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &install_driver,
            &InstallOptions::default(),
        )
        .await;

        println!("== RESULT: {:?}", result.clobbered_paths);

        assert_check_files(
            target_prefix.path(),
            &[
                "clobber.txt",
                "clobber.txt__clobber-from-clobber-2",
                "clobber.txt__clobber-from-clobber-3",
                "another-clobber.txt",
                "another-clobber.txt__clobber-from-clobber-3",
            ],
        );

        // content of  clobber.txt
        assert_eq!(
            fs::read_to_string(target_prefix.path().join("clobber.txt")).unwrap(),
            "clobber-1 v2\n"
        );
        assert_eq!(
            fs::read_to_string(target_prefix.path().join("another-clobber.txt")).unwrap(),
            "clobber-2\n"
        );
    }

    #[tokio::test]
    async fn test_self_clobber_update() {
        // Create a transaction
        let repodata_record_1 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-1-0.1.0-h4616a5c_0.tar.bz2"),
        );

        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations: vec![TransactionOperation::Install(repodata_record_1.clone())],
            python_info: None,
            current_python_info: None,
            platform: Platform::current(),
        };

        // execute transaction
        let target_prefix = tempfile::tempdir().unwrap();

        let packages_dir = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        execute_transaction(
            transaction,
            target_prefix.path(),
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &InstallDriver::default(),
            &InstallOptions::default(),
        )
        .await;

        // check that the files are there
        assert_check_files(
            target_prefix.path(),
            &["clobber.txt", "another-clobber.txt"],
        );

        let mut prefix_records = PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();
        prefix_records.sort_by(|a, b| {
            a.repodata_record
                .package_record
                .name
                .as_normalized()
                .cmp(b.repodata_record.package_record.name.as_normalized())
        });

        // Reinstall the same package
        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations: vec![TransactionOperation::Change {
                old: prefix_records[0].clone(),
                new: repodata_record_1,
            }],
            python_info: None,
            current_python_info: None,
            platform: Platform::current(),
        };

        let install_driver = InstallDriver::builder()
            .with_prefix_records(&prefix_records)
            .finish();

        install_driver
            .pre_process(&transaction, target_prefix.path())
            .unwrap();
        let dl_client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());
        for op in &transaction.operations {
            execute_operation(
                target_prefix.path(),
                &dl_client,
                &cache,
                &install_driver,
                op.clone(),
                &InstallOptions::default(),
            )
            .await;
        }

        // Check what files are in the prefix now (note that unclobbering wasn't run yet)
        // But also, this is a reinstall so the files should just be overwritten.
        assert_check_files(
            target_prefix.path(),
            &["clobber.txt", "another-clobber.txt"],
        );
    }

    #[tokio::test]
    async fn test_clobber_update_and_remove() {
        // Create a transaction
        let operations = test_operations();

        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations,
            python_info: None,
            current_python_info: None,
            platform: Platform::current(),
        };

        // execute transaction
        let target_prefix = tempfile::tempdir().unwrap();

        let packages_dir = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        execute_transaction(
            transaction,
            target_prefix.path(),
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &InstallDriver::default(),
            &InstallOptions::default(),
        )
        .await;

        // check that the files are there
        assert_check_files(
            target_prefix.path(),
            &[
                "clobber.txt",
                "clobber.txt__clobber-from-clobber-2",
                "clobber.txt__clobber-from-clobber-3",
                "another-clobber.txt",
                "another-clobber.txt__clobber-from-clobber-2",
                "another-clobber.txt__clobber-from-clobber-3",
            ],
        );

        let mut prefix_records = PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();
        prefix_records.sort_by(|a, b| {
            a.repodata_record
                .package_record
                .name
                .as_normalized()
                .cmp(b.repodata_record.package_record.name.as_normalized())
        });

        let update_ops = test_operations_update();

        // remove one of the clobbering files
        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations: vec![
                TransactionOperation::Change {
                    old: prefix_records[2].clone(),
                    new: update_ops[2].clone(),
                },
                TransactionOperation::Remove(prefix_records[0].clone()),
                TransactionOperation::Remove(prefix_records[1].clone()),
            ],
            python_info: None,
            current_python_info: None,
            platform: Platform::current(),
        };

        let prefix_records = PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();
        let install_driver = InstallDriver::builder()
            .with_prefix_records(&prefix_records)
            .finish();

        execute_transaction(
            transaction,
            target_prefix.path(),
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &install_driver,
            &InstallOptions::default(),
        )
        .await;

        assert_check_files(target_prefix.path(), &["clobber.txt"]);

        // content of  clobber.txt
        assert_eq!(
            fs::read_to_string(target_prefix.path().join("clobber.txt")).unwrap(),
            "clobber-3 v2\n"
        );

        let update_ops = test_operations_update();

        // remove one of the clobbering files
        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations: vec![TransactionOperation::Install(update_ops[0].clone())],
            python_info: None,
            current_python_info: None,
            platform: Platform::current(),
        };

        let prefix_records = PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();
        let install_driver = InstallDriver::builder()
            .with_prefix_records(&prefix_records)
            .finish();

        execute_transaction(
            transaction,
            target_prefix.path(),
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &install_driver,
            &InstallOptions::default(),
        )
        .await;

        assert_check_files(
            target_prefix.path(),
            &["clobber.txt", "clobber.txt__clobber-from-clobber-3"],
        );

        // content of  clobber.txt
        assert_eq!(
            fs::read_to_string(target_prefix.path().join("clobber.txt")).unwrap(),
            "clobber-1 v2\n"
        );
    }

    #[tokio::test]
    async fn test_clobber_python_noarch() {
        // Create a transaction
        let operations = test_python_noarch_operations();

        let python_info = PythonInfo::from_version(
            &Version::from_str("3.11.0").unwrap(),
            None,
            Platform::current(),
        )
        .unwrap();
        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations,
            python_info: Some(python_info.clone()),
            current_python_info: Some(python_info.clone()),
            platform: Platform::current(),
        };

        // execute transaction
        let target_prefix = tempfile::tempdir().unwrap();

        let packages_dir = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        let install_options = InstallOptions {
            python_info: Some(python_info.clone()),
            ..Default::default()
        };

        execute_transaction(
            transaction,
            target_prefix.path(),
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &InstallDriver::default(),
            &install_options,
        )
        .await;

        // check that the files are there
        if cfg!(unix) {
            assert_check_files(
                &target_prefix
                    .path()
                    .join("lib/python3.11/site-packages/clobber"),
                &["clobber.py", "clobber.py__clobber-from-clobber-pynoarch-2"],
            );
        } else {
            assert_check_files(
                &target_prefix.path().join("Lib/site-packages/clobber"),
                &["clobber.py", "clobber.py__clobber-from-clobber-pynoarch-2"],
            );
        }
    }

    // This used to hit an expect in the clobbering code
    #[tokio::test]
    async fn test_transaction_with_clobber_remove_all() {
        let operations = test_operations();

        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations,
            python_info: None,
            current_python_info: None,
            platform: Platform::current(),
        };

        // execute transaction
        let target_prefix = tempfile::tempdir().unwrap();

        let packages_dir = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        execute_transaction(
            transaction,
            target_prefix.path(),
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &InstallDriver::default(),
            &InstallOptions::default(),
        )
        .await;

        let prefix_records = PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();

        // remove one of the clobbering files
        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations: prefix_records
                .iter()
                .map(|r| TransactionOperation::Remove(r.clone()))
                .collect(),
            python_info: None,
            current_python_info: None,
            platform: Platform::current(),
        };

        let install_driver = InstallDriver::builder()
            .with_prefix_records(&prefix_records)
            .finish();

        execute_transaction(
            transaction,
            target_prefix.path(),
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &install_driver,
            &InstallOptions::default(),
        )
        .await;

        assert_check_files(target_prefix.path(), &[]);
    }

    // This used to hit an expect in the clobbering code
    #[tokio::test]
    async fn test_dependency_clobber() {
        // Create a transaction
        let repodata_record_1 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-python-0.1.0-cpython.conda"),
        );
        let repodata_record_2 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-python-0.1.0-pypy.conda"),
        );
        let repodata_record_3 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-pypy-0.1.0-h4616a5c_0.conda"),
        );

        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations: vec![TransactionOperation::Install(repodata_record_1)],
            python_info: None,
            current_python_info: None,
            platform: Platform::current(),
        };

        // execute transaction
        let target_prefix = tempfile::tempdir().unwrap();

        let packages_dir = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        execute_transaction(
            transaction,
            target_prefix.path(),
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &InstallDriver::default(),
            &InstallOptions::default(),
        )
        .await;

        let prefix_records = PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();

        // remove one of the clobbering files
        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations: vec![
                TransactionOperation::Change {
                    old: prefix_records[0].clone(),
                    new: repodata_record_2,
                },
                TransactionOperation::Install(repodata_record_3),
            ],
            python_info: None,
            current_python_info: None,
            platform: Platform::current(),
        };

        let install_driver = InstallDriver::builder()
            .with_prefix_records(&prefix_records)
            .finish();

        install_driver
            .pre_process(&transaction, target_prefix.path())
            .unwrap();

        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());
        for op in &transaction.operations {
            execute_operation(
                target_prefix.path(),
                &client,
                &cache,
                &install_driver,
                op.clone(),
                &InstallOptions::default(),
            )
            .await;
        }

        // check that `bin/python` was installed as a single clobber file
        assert_check_files(
            &target_prefix.path().join("bin"),
            &["python__clobber-from-clobber-pypy"],
        );

        install_driver
            .post_process(&transaction, target_prefix.path())
            .unwrap();

        assert_check_files(&target_prefix.path().join("bin"), &["python"]);
    }
}
