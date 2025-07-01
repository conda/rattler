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
    PackageName, PrefixRecord,
    package::{IndexJson, PathsEntry},
    prefix_record,
};

use super::package_path_resolver::*;

const CLOBBER_TEMPLATE: &str = "__clobber-from-";

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

/// A registry for clobbering files
/// The registry keeps track of all files that are installed by a package and
/// can be used to rename files that are already installed by another package.
#[derive(Debug, Default, Clone)]
pub struct ClobberRegistry {
    /// A cache and priority list of package names.
    package_names: Vec<PackageName>,

    /// Resolver responsible for detecting clobbers (collisions).
    path_resolver: PackagePathResolver,

    /// Store conflicts that we found.
    ///
    /// We need this to report only new conflicts in return value of `register_paths`.
    conflicts: Vec<Conflict>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
struct PackageNameIdx(usize);

impl ClobberRegistry {
    /// Create a new clobber registry that is initialized with the given prefix
    /// records.
    pub fn new<'i>(prefix_records: impl IntoIterator<Item = &'i PrefixRecord>) -> Self {
        let mut registry = ClobberRegistry::default();

        for prefix_record in prefix_records {
            let name = prefix_record.name();
            let paths = prefix_record.paths_data.paths.iter().map(|pe| pe.path());

            registry.register_paths_by_name(name, paths);
        }

        registry
    }

    /// Register that all the paths of a package are being removed.
    pub fn unregister_paths(&mut self, prefix_paths: &PrefixRecord) {
        let package_name = prefix_paths.repodata_record.package_record.name.clone();
        // Assume that normalized name in different two different PackageName are unique.
        let normalized_name = package_name.as_normalized();
        self.path_resolver.remove_package(normalized_name);
        // Since we're need to report new conflicts only in
        // `register_paths` we can simply copy current state of
        // conflicts from the `path_resolver` as its `remove_package`
        // method will recalculate conflicts in package removal if
        // some conflict existed before.
        self.sync_conflicts();
    }

    /// Check if we have the package name already registered, otherwise register it.
    fn get_or_insert_name_index(&mut self, name: &PackageName) -> PackageNameIdx {
        if let Some(idx) = self.package_names.iter().position(|n| n == name) {
            PackageNameIdx(idx)
        } else {
            self.package_names.push(name.clone());
            PackageNameIdx(self.package_names.len() - 1)
        }
    }

    // /// Returns `PackageName` from the `PackageNameIdx`.
    // fn get_package_name(&self, idx: PackageNameIdx) -> Option<&PackageName> {
    //     self.package_names.get(idx.0)
    // }

    /// Returns `PackageName` from the package normalized name.
    fn get_package_name_from_normalized(&self, name: &str) -> Option<&PackageName> {
        self.package_names
            .iter()
            .find(|p| p.as_normalized() == name)
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
        computed_paths: &[(PathsEntry, PathBuf)],
    ) -> HashMap<PathBuf, PathBuf> {
        let name = &index_json.name;

        self.register_paths_by_name(name, computed_paths.iter().map(|p| &p.1))
    }

    fn register_paths_by_name<'a>(
        &'a mut self,
        name: &PackageName,
        computed_paths: impl IntoIterator<Item = &'a PathBuf>,
    ) -> HashMap<PathBuf, PathBuf> {
        let normalized_name = name.as_normalized();
        let package_idx = self.get_or_insert_name_index(name);

        let paths = self.construct_paths_for_resolver(computed_paths);

        self.path_resolver
            .add_package(normalized_name, package_idx.0, &paths);

        // We want to update conflicts in path resovler to obtain
        // difference for correct link paths.
        self.path_resolver.resolve_conflicts();

        // Map that will be used to link file to appropriate place to
        // avoid writing two different files to the same place.
        self.get_conflicts_link_paths()
    }

    /// Compute conflict difference and return map from original package path to link path.
    ///
    /// For example if package A has path `a/b` and another package B
    /// has path `a/b`, then second one will be linked to
    /// `a/b__clobber-from-B`.
    fn get_conflicts_link_paths(&self) -> HashMap<PathBuf, PathBuf> {
        // TODO: Store conflicting paths under __clobbers__/pkg/
        // directory.  This will make merging of directories a great
        // deal easier in comparison with previous approach.
        todo!()
    }

    /// Syncronize conflicts state with `path_resolver`.
    fn sync_conflicts(&mut self) {
        self.conflicts = self.path_resolver.get_conflicts().into();
    }

    /// Construct vector of paths accepted by path resolver `add_package` method.
    ///
    /// For resolver to work properly we also have to store all ancestors of each path.
    fn construct_paths_for_resolver<'a>(
        &self,
        initial_paths: impl IntoIterator<Item = &'a PathBuf>,
    ) -> Vec<(&'a Path, EntryType)> {
        initial_paths
            .into_iter()
            .flat_map(|p: &PathBuf| {
                let mut it = p.ancestors();
                let file = it.next().map(|p| (p, EntryType::File));
                let parents = it
                    .filter(|&p| !p.as_os_str().is_empty())
                    .map(|p| (p, EntryType::Directory))
                    .collect::<Vec<_>>();
                parents.into_iter().rev().chain(file)
            })
            .collect()
    }

    /// Unclobber the paths after all installation steps have been completed.
    /// Returns an overview of all the clobbered files.
    pub fn unclobber(
        &mut self,
        sorted_prefix_records: &[&PrefixRecord],
        target_prefix: &Path,
    ) -> Result<HashMap<PathBuf, ClobberedPath>, ClobberError> {
        // Needed for step 7.
        let mut prefix_records_to_rewrite = vec![]; // TODO

        // TODO: There are several steps that we have to do, and not all of them are currently implemented.
        //
        // 1. [x] Save current priorities from path resolver in case unclobber will be called second time.
        // 2. [x] Reprioritize based on the given sorted_prefix_records to get correct paths in the final layout.
        // 3. [x] Resolve paths based on the path resolver conflicts.
        // 4. [x] Restore original priorities.
        // 5. [ ] Swap current owner (first one who took ownership) of path with winner.
        // 6. [x] Compute result of unclobbering.
        // 7. [ ] Write conda-meta file.
        // 8. [x] Return result of unclobbering.

        // 1
        let original_priorities = self.path_resolver.priorities();

        // 2
        let new_priorities = sorted_prefix_records
            .iter()
            .enumerate()
            .map(|(priority, &pr)| {
                let name = pr.name().as_normalized().to_string();
                (name, priority)
            })
            .collect();
        self.path_resolver.reprioritize(&new_priorities);

        // 3
        self.path_resolver.resolve_conflicts();

        // 4
        self.path_resolver.reprioritize(&original_priorities);

        // 6
        let result = self
            .path_resolver
            .get_conflicts()
            .iter()
            .map(|conflict| self.conflict_to_clobbered_path(conflict))
            .collect();

        // 7
        self.update_conda_meta(target_prefix, &prefix_records_to_rewrite)?;

        // 8
        Ok(result)
    }

    /// Helper function to preserve compatability.
    fn conflict_to_clobbered_path(&self, conflict: &Conflict) -> (PathBuf, ClobberedPath) {
        let path = conflict.path.clone();

        let package = self
            .get_package_name_from_normalized(&conflict.winner.package_name)
            .unwrap()
            .clone();
        let other_packages = conflict
            .losers
            .iter()
            .map(|e| {
                self.get_package_name_from_normalized(&e.package_name)
                    .unwrap()
                    .clone()
            })
            .collect();
        let clobbered_path = ClobberedPath {
            package,
            other_packages,
        };
        (path, clobbered_path)
    }

    /// Update conda metadata on disk with new file tree obtained
    /// after during unclobbering.
    fn update_conda_meta(
        &self,
        target_prefix: &Path,
        prefix_records_to_rewrite: &[&PrefixRecord], // Maybe impl IntoIterator?
    ) -> Result<(), ClobberError> {
        let conda_meta_path = target_prefix.join("conda-meta");

        // for idx in prefix_records_to_rewrite {
        //     let rec = &prefix_records[idx];
        //     tracing::debug!(
        //         "writing updated prefix record to: {:?}",
        //         conda_meta.join(rec.file_name())
        //     );
        //     rec.write_to_path(conda_meta.join(rec.file_name()), true)
        //         .map_err(|e| {
        //             ClobberError::IoError(
        //                 format!("failed to write updated prefix record {}", rec.file_name()),
        //                 e,
        //             )
        //         })?;
        // }

        todo!()
    }
}

fn clobber_name(path: &Path, package_name: &PackageName) -> PathBuf {
    let file_name = path.file_name().unwrap_or_default();
    let mut new_path = path.to_path_buf();
    new_path.set_file_name(format!(
        "{}{CLOBBER_TEMPLATE}{}",
        file_name.to_string_lossy(),
        package_name.as_normalized(),
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
            path.original_path = new_path_is_clobber.then(|| old_path.to_path_buf());
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
    use rattler_conda_types::{Platform, PrefixRecord, RepoDataRecord, Version, prefix::Prefix};
    use transaction::TransactionOperation;

    use crate::{
        get_repodata_record, get_test_data_dir,
        install::{InstallDriver, InstallOptions, PythonInfo, test_utils::*, transaction},
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
        let prefix_path = Prefix::create(target_prefix.path()).unwrap();

        let packages_dir = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        execute_transaction(
            transaction,
            &prefix_path,
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
            &prefix_path,
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
            operations.shuffle(&mut rand::rng());

            let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
                operations,
                python_info: None,
                current_python_info: None,
                platform: Platform::current(),
            };

            // execute transaction
            let target_prefix = tempfile::tempdir().unwrap();
            let prefix_path = Prefix::create(target_prefix.path()).unwrap();

            let packages_dir = tempfile::tempdir().unwrap();
            let cache = PackageCache::new(packages_dir.path());

            execute_transaction(
                transaction,
                &prefix_path,
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

            let prefix_records: Vec<PrefixRecord> =
                PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();

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
            operations.shuffle(&mut rand::rng());

            let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
                operations,
                python_info: None,
                current_python_info: None,
                platform: Platform::current(),
            };

            // execute transaction
            let target_prefix = tempfile::tempdir().unwrap();
            let prefix_path = Prefix::create(target_prefix.path()).unwrap();

            let packages_dir = tempfile::tempdir().unwrap();
            let cache = PackageCache::new(packages_dir.path());

            execute_transaction(
                transaction,
                &prefix_path,
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
                &prefix_path,
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
        let prefix_path = Prefix::create(target_prefix.path()).unwrap();

        let packages_dir = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        execute_transaction(
            transaction,
            &prefix_path,
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

        let mut prefix_records: Vec<PrefixRecord> =
            PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();
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
            &prefix_path,
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
        let prefix_path = Prefix::create(target_prefix.path()).unwrap();

        let packages_dir = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        execute_transaction(
            transaction,
            &prefix_path,
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

        let mut prefix_records: Vec<PrefixRecord> =
            PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();
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
                &prefix_path,
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
        let prefix_path = Prefix::create(target_prefix.path()).unwrap();

        let packages_dir = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        execute_transaction(
            transaction,
            &prefix_path,
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

        let mut prefix_records: Vec<PrefixRecord> =
            PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();
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
            &prefix_path,
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
            &prefix_path,
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
        let prefix_path = Prefix::create(target_prefix.path()).unwrap();

        let packages_dir = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        let install_options = InstallOptions {
            python_info: Some(python_info.clone()),
            ..Default::default()
        };

        execute_transaction(
            transaction,
            &prefix_path,
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
        let prefix_path = Prefix::create(target_prefix.path()).unwrap();

        let packages_dir = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        execute_transaction(
            transaction,
            &prefix_path,
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &InstallDriver::default(),
            &InstallOptions::default(),
        )
        .await;

        let prefix_records: Vec<PrefixRecord> =
            PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();

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
            &prefix_path,
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
        let prefix_path = Prefix::create(target_prefix.path()).unwrap();

        let packages_dir = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        execute_transaction(
            transaction,
            &prefix_path,
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &InstallDriver::default(),
            &InstallOptions::default(),
        )
        .await;

        let prefix_records: Vec<PrefixRecord> =
            PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();

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
            .pre_process(&transaction, &prefix_path)
            .unwrap();

        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());
        for op in &transaction.operations {
            execute_operation(
                &prefix_path,
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
            .post_process(&transaction, &prefix_path)
            .unwrap();

        assert_check_files(&target_prefix.path().join("bin"), &["python"]);
    }

    #[tokio::test]
    async fn test_dir_to_file_upgrade() {
        let repodata_record_1 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-directory-upgrade-0.1.0-h4616a5c_0.conda"),
        );
        let repodata_record_2 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-directory-upgrade-0.2.0-h4616a5c_0.conda"),
        );

        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations: vec![TransactionOperation::Install(repodata_record_1)],
            python_info: None,
            current_python_info: None,
            platform: Platform::current(),
        };

        // execute transaction
        let target_prefix = tempfile::tempdir().unwrap();
        let prefix_path = Prefix::create(target_prefix.path()).unwrap();

        let packages_dir = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        execute_transaction(
            transaction,
            &prefix_path,
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &InstallDriver::default(),
            &InstallOptions::default(),
        )
        .await;

        assert!(target_prefix.path().join("xbin").is_dir());

        let prefix_records: Vec<PrefixRecord> =
            PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();

        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations: vec![TransactionOperation::Change {
                old: prefix_records[0].clone(),
                new: repodata_record_2,
            }],
            python_info: None,
            current_python_info: None,
            platform: Platform::current(),
        };

        let install_driver = InstallDriver::builder()
            .with_prefix_records(&prefix_records)
            .finish();

        execute_transaction(
            transaction,
            &prefix_path,
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &install_driver,
            &InstallOptions::default(),
        )
        .await;

        assert!(target_prefix.path().join("xbin").is_file());
    }

    // // This used to hit an expect in the clobbering code
    // #[tokio::test]
    // async fn test_directory_clobber() {
    //     // Create a transaction
    //     let repodata_record_with_dir = get_repodata_record(
    //         get_test_data_dir().join("clobber/clobber-with-dir-0.1.0-h4616a5c_0.conda"),
    //     );
    //     let repodata_record_without_dir = get_repodata_record(
    //         get_test_data_dir().join("clobber/clobber-without-dir-0.1.0-h4616a5c_0.conda"),
    //     );

    //     // let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
    //     //     operations: vec![TransactionOperation::Install(repodata_record_with_dir)],
    //     //     python_info: None,
    //     //     current_python_info: None,
    //     //     platform: Platform::current(),
    //     // };

    //     // execute transaction
    //     let target_prefix = tempfile::tempdir().unwrap();
    //     let prefix_path = Prefix::create(target_prefix.path()).unwrap();
    //     let packages_dir = tempfile::tempdir().unwrap();
    //     let cache = PackageCache::new(packages_dir.path());

    //     // execute_transaction(
    //     //     transaction,
    //     //     &prefix_path,
    //     //     &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
    //     //     &cache,
    //     //     &InstallDriver::default(),
    //     //     &InstallOptions::default(),
    //     // )
    //     // .await;

    //     // let prefix_records: Vec<PrefixRecord> =
    //     //     PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();

    //     // remove one of the clobbering files
    //     let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
    //         operations: vec![
    //             // TransactionOperation::Remove(prefix_records[0].clone()),
    //             TransactionOperation::Install(repodata_record_with_dir),
    //             TransactionOperation::Install(repodata_record_without_dir),
    //         ],
    //         python_info: None,
    //         current_python_info: None,
    //         platform: Platform::current(),
    //     };

    //     let install_driver = InstallDriver::builder().with_prefix_records(&[]).finish();

    //     execute_transaction(
    //         transaction,
    //         &prefix_path,
    //         &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
    //         &cache,
    //         &install_driver,
    //         &InstallOptions::default(),
    //     )
    //     .await;

    //     // assert!(target_prefix.path().join("dir").is_dir())

    //     assert_check_files(&target_prefix.path(), &["dir"]);
    // }
}
