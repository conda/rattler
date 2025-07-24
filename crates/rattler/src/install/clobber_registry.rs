//! Implements a registry for "clobbering" files (files that are appearing in
//! multiple packages)

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use path_resolver::{FromClobbers, PathResolver};
use rattler_conda_types::{
    package::{IndexJson, PathsEntry},
    PackageName, PackageRecord, PrefixRecord,
};

pub const CLOBBERS_DIR_NAME: &str = "__clobbers__";

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
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ClobberRegistry {
    /// Map that used to map normalized names to their original
    /// `rattler_conda_types::PackageName` values. This is needed to
    /// keep API compatible.
    package_name_map: HashMap<path_resolver::PackageName, rattler_conda_types::PackageName>,

    /// Trie responsible for storing conflicting paths and resolving
    /// them.
    path_trie: PathResolver,

    /// Path to clobbered package files that we want to move from the
    /// next winner in clobbers after previous was removed.
    to_add: FromClobbers,
}

impl ClobberRegistry {
    /// Create a new clobber registry that is initialized with the given prefix
    /// records.
    pub fn new<'i>(prefix_records: impl IntoIterator<Item = &'i PrefixRecord>) -> Self {
        use rattler_conda_types::prefix_record::PathsEntry;

        let mut registry = ClobberRegistry::default();

        let prefix_records =
            PackageRecord::sort_topologically(prefix_records.into_iter().collect::<Vec<_>>());

        for prefix_record in prefix_records.into_iter().rev() {
            let name = prefix_record.name();
            let paths = prefix_record.paths_data.paths.iter().map(PathsEntry::path);

            registry.register_paths_by_name(name, paths);
        }

        registry
    }

    /// Register that all the paths of a package are being removed.
    pub fn unregister_paths(&mut self, prefix_paths: &PrefixRecord) {
        let package_name = prefix_paths.repodata_record.package_record.name.clone();
        // Assume that normalized name in different two different PackageName are unique.
        let normalized_name = package_name.as_normalized();

        let (_, to_add) = self.path_trie.unregister_package(normalized_name);

        self.package_name_map.remove(normalized_name);

        self.to_add.extend(to_add);
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

        let computed_paths = computed_paths.into_iter().collect::<Vec<_>>();
        let immediate_conflicts = self
            .path_trie
            .insert_package(normalized_name.into(), computed_paths.as_slice());

        // Map that will be used to link file to appropriate place to
        // avoid writing two different files to the same place.
        let link_paths = immediate_conflicts
            .into_iter()
            .map(|p| (p.clone(), get_clobber_relative_path(normalized_name, &p)))
            .collect();

        self.package_name_map
            .insert(normalized_name.into(), name.clone());

        link_paths
    }

    /// Unclobber the paths after all installation steps have been completed.
    /// Returns an overview of all the clobbered files.
    ///
    /// There are several steps that we are doing:
    /// 1. Reprioritize based on the given `sorted_prefix_records` to get corrected paths for the final layout.
    /// 2. Synchronize in-memory representation with what we have on-disk.
    /// 3. Update conda-meta file.
    /// 4. Return result of unclobbering.
    pub fn unclobber(
        &mut self,
        sorted_prefix_records: &[&PrefixRecord],
        target_prefix: &Path,
    ) -> Result<HashMap<PathBuf, ClobberedPath>, ClobberError> {
        // We store copy of prefix records, so we can update them later on.
        // They will be used to update metadata.
        let mut prefix_records: HashMap<&str, PrefixRecord> = sorted_prefix_records
            .iter()
            .map(|&pr| (pr.name().as_normalized(), pr.clone()))
            .collect();
        let mut prefix_records_to_rewrite: HashSet<&str> = HashSet::new();

        // 1
        let new_priorities = sorted_prefix_records
            .iter()
            .map(|&pr| pr.name().as_normalized().to_string())
            .collect::<Vec<String>>();

        let (mut removals, additions) = self.path_trie.reprioritize_packages(new_priorities);

        let more_additions = self
            .to_add
            .iter()
            .filter(|&p| self.path_trie.packages().contains(&p.1) && !additions.contains(p));

        let mut all_additions = more_additions
            .chain(additions.iter())
            .cloned()
            .collect::<Vec<_>>();

        // Elements that we want to first remove and then add.
        //
        // This not only saves on-disk operations but also required to
        // handle updates correctly.
        let mut duplicates = vec![];
        removals.retain(|r| {
            let duplicate = all_additions.contains(r);
            if duplicate {
                duplicates.push(r.clone());
            }
            !duplicate
        });
        all_additions.retain(|a| !duplicates.contains(a));

        // 2
        PathResolver::sync_clobbers(
            target_prefix,
            &target_prefix.join(CLOBBERS_DIR_NAME),
            &removals,
            &all_additions,
        )
        .map_err(|e| ClobberError::IoError("On-disk syncornization error".into(), e))?;

        // 3
        for (path, pkg) in &removals {
            let prefix_record = prefix_records.get_mut(pkg.as_str()).unwrap();
            let clobber_path = Path::new(CLOBBERS_DIR_NAME).join(pkg).join(path);
            rename_path_in_prefix_record(prefix_record, path, &clobber_path, true);
            prefix_records_to_rewrite.insert(pkg.as_str());
        }

        for (path, pkg) in &all_additions {
            let prefix_record = prefix_records.get_mut(pkg.as_str()).unwrap();
            let clobber_path = Path::new(CLOBBERS_DIR_NAME).join(pkg).join(path);
            rename_path_in_prefix_record(prefix_record, &clobber_path, path, false);
            prefix_records_to_rewrite.insert(pkg.as_str());
        }
        Self::update_conda_meta(target_prefix, &prefix_records, &prefix_records_to_rewrite)?;

        // 4
        let clobbered_paths = self
            .path_trie
            .collect_clobbered_paths()
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    ClobberedPath {
                        package: self.package_name_map.get(&v.winner).unwrap().clone(),
                        other_packages: v
                            .losers
                            .iter()
                            .map(|loser| self.package_name_map.get(loser).unwrap().clone())
                            .collect(),
                    },
                )
            })
            .collect();
        Ok(clobbered_paths)
    }

    /// Update conda metadata on disk with new file tree obtained
    /// after during unclobbering.
    fn update_conda_meta(
        target_prefix: &Path,
        prefix_records: &HashMap<&str, PrefixRecord>,
        prefix_records_to_rewrite: &HashSet<&str>,
    ) -> Result<(), ClobberError> {
        let conda_meta_path = target_prefix.join("conda-meta");

        for idx in prefix_records_to_rewrite {
            let record = &prefix_records[*idx];
            tracing::debug!(
                "writing updated prefix record to: {:?}",
                conda_meta_path.join(record.file_name())
            );
            record
                .write_to_path(conda_meta_path.as_path().join(record.file_name()), true)
                .map_err(|e| {
                    ClobberError::IoError(
                        format!(
                            "failed to write updated prefix record {}",
                            record.file_name()
                        ),
                        e,
                    )
                })?;
        }

        Ok(())
    }
}

/// Constructs a relative path to clobber.
fn get_clobber_relative_path(package_name: &str, relative_file_path: &Path) -> PathBuf {
    Path::new(CLOBBERS_DIR_NAME)
        .join(package_name)
        .join(relative_file_path)
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
        ffi::OsStr,
        path::{Path, PathBuf},
        str::FromStr,
    };

    use fs_err as fs;
    use insta::assert_yaml_snapshot;
    use itertools::Itertools;
    use rattler_conda_types::{prefix::Prefix, Platform, PrefixRecord, RepoDataRecord, Version};
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

    fn collect_paths_to_files(target_prefix: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();

        for entry in walkdir::WalkDir::new(target_prefix)
            .into_iter()
            .filter_map(Result::ok)
        {
            let path = entry.path();

            if path
                .components()
                .map(std::path::Component::as_os_str)
                .any(|c| c == OsStr::new("conda-meta"))
            {
                continue;
            }
            if path.is_file() {
                // strip off the base directory so we can compare to our hard-coded list
                let rel = path
                    .strip_prefix(target_prefix)
                    .expect("base dir is prefix")
                    .to_owned();
                files.push(rel);
            }
        }

        files.into_iter().sorted().dedup().collect()
    }

    macro_rules! assert_check_files {
        ($target_prefix:expr, $expected_files:expr $(,)?) => {
            let target_prefix: &Path = $target_prefix;
            let expected_files: &[&str] = $expected_files;

            let expected_paths = expected_files
                .into_iter()
                .map(|n| PathBuf::from(n))
                .sorted()
                .dedup()
                .collect::<Vec<PathBuf>>();
            let actual_paths = collect_paths_to_files(target_prefix);
            // TODO: use snapshot testing.
            assert_eq!(actual_paths, expected_paths)
        };
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
        assert_check_files!(
            target_prefix.path(),
            &[
                "clobber.txt",
                "another-clobber.txt",
                "__clobbers__/clobber-2/clobber.txt",
                "__clobbers__/clobber-2/another-clobber.txt",
                "__clobbers__/clobber-3/clobber.txt",
                "__clobbers__/clobber-3/another-clobber.txt",
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

        assert_check_files!(
            target_prefix.path(),
            &[
                "clobber.txt",
                "another-clobber.txt",
                "__clobbers__/clobber-3/clobber.txt",
                "__clobbers__/clobber-3/another-clobber.txt",
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
                    PathBuf::from("__clobbers__/clobber-3/another-clobber.txt"),
                    PathBuf::from("__clobbers__/clobber-3/clobber.txt")
                ]
        );

        assert_eq!(prefix_record_clobber_3.paths_data.paths.len(), 2);
        assert_yaml_snapshot!(prefix_record_clobber_3.paths_data);
    }

    #[tokio::test]
    async fn test_all_possible_orders_clobber() {
        let test_operations = test_operations();
        let len = test_operations.len();
        for operations in test_operations.into_iter().permutations(len) {
            let operations = operations.clone();

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
            assert_check_files!(
                target_prefix.path(),
                &[
                    "clobber.txt",
                    "another-clobber.txt",
                    "__clobbers__/clobber-2/clobber.txt",
                    "__clobbers__/clobber-2/another-clobber.txt",
                    "__clobbers__/clobber-3/clobber.txt",
                    "__clobbers__/clobber-3/another-clobber.txt",
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
                            PathBuf::from("__clobbers__/clobber-2/another-clobber.txt"),
                            PathBuf::from("__clobbers__/clobber-2/clobber.txt")
                        ]
                    );
                } else if record.repodata_record.package_record.name.as_normalized() == "clobber-3"
                {
                    assert_eq!(
                        record.files,
                        vec![
                            PathBuf::from("__clobbers__/clobber-3/another-clobber.txt"),
                            PathBuf::from("__clobbers__/clobber-3/clobber.txt")
                        ]
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn test_all_possible_orders_clobber_nested() {
        let test_operations = test_operations_nested();
        let len = test_operations.len();
        let operations_permutations = test_operations.into_iter().permutations(len);

        for operations in operations_permutations {
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
            assert_check_files!(
                &target_prefix.path(),
                &[
                    "clobber/bobber/clobber.txt",
                    "__clobbers__/clobber-nested-1/clobber/bobber/clobber.txt",
                    "__clobbers__/clobber-nested-3/clobber/bobber/clobber.txt",
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
                    "__clobbers__/clobber-nested-3/clobber/bobber/clobber.txt"
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

            assert_check_files!(
                &target_prefix.path(),
                &[
                    "__clobbers__/clobber-nested-3/clobber/bobber/clobber.txt",
                    "clobber/bobber/clobber.txt"
                ],
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
        assert_check_files!(
            target_prefix.path(),
            &[
                "clobber.txt",
                "another-clobber.txt",
                "__clobbers__/clobber-2/clobber.txt",
                "__clobbers__/clobber-2/another-clobber.txt",
                "__clobbers__/clobber-3/clobber.txt",
                "__clobbers__/clobber-3/another-clobber.txt",
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

        assert_check_files!(
            target_prefix.path(),
            &[
                "clobber.txt",
                "another-clobber.txt",
                "__clobbers__/clobber-2/clobber.txt",
                "__clobbers__/clobber-3/clobber.txt",
                "__clobbers__/clobber-3/another-clobber.txt",
            ],
        );

        // content of clobber.txt
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
        assert_check_files!(
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
        assert_check_files!(
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
        assert_check_files!(
            target_prefix.path(),
            &[
                "clobber.txt",
                "another-clobber.txt",
                "__clobbers__/clobber-2/clobber.txt",
                "__clobbers__/clobber-2/another-clobber.txt",
                "__clobbers__/clobber-3/clobber.txt",
                "__clobbers__/clobber-3/another-clobber.txt",
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

        assert_check_files!(target_prefix.path(), &["clobber.txt"]);

        // content of clobber.txt
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

        assert_check_files!(
            target_prefix.path(),
            &["clobber.txt", "__clobbers__/clobber-3/clobber.txt"],
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
            assert_check_files!(
                &target_prefix.path(),
                &[
                    "lib/python3.11/site-packages/clobber/clobber.py",
                    "__clobbers__/clobber-pynoarch-2/lib/python3.11/site-packages/clobber/clobber.py",
                ],
            );
        } else {
            assert_check_files!(
                &target_prefix.path(),
                &[
                    "Lib/site-packages/clobber/clobber.py",
                    "__clobbers__/clobber-pynoarch-2/Lib/site-packages/clobber/clobber.py",
                ],
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

        assert_check_files!(target_prefix.path(), &[]);
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

        // Before it check that `bin/python` was installed as a single
        // clobber file, but now we can resolve this beforehand, so we
        // just check if file is installed.
        assert_check_files!(
            &target_prefix.path(),
            &["bin/python"], // "__clobbers__/clobber-pypy/bin/python"
        );

        install_driver
            .post_process(&transaction, &prefix_path)
            .unwrap();

        assert_check_files!(&target_prefix.path(), &["bin/python"]);
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

    // This used to hit an expect in the clobbering code
    #[tokio::test]
    async fn test_direct_file_and_directory_clobber() {
        // Create a transaction
        let repodata_record_with_dir = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-with-dir-0.1.0-h4616a5c_0.conda"),
        );
        let repodata_record_without_dir = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-without-dir-0.1.0-h4616a5c_0.conda"),
        );

        // execute transaction
        let target_prefix = tempfile::tempdir().unwrap();
        let prefix_path = Prefix::create(target_prefix.path()).unwrap();
        let packages_dir = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        // remove one of the clobbering files
        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations: vec![
                // TransactionOperation::Remove(prefix_records[0].clone()),
                TransactionOperation::Install(repodata_record_with_dir),
                TransactionOperation::Install(repodata_record_without_dir),
            ],
            python_info: None,
            current_python_info: None,
            platform: Platform::current(),
        };

        let install_driver = InstallDriver::builder().with_prefix_records(&[]).finish();

        execute_transaction(
            transaction,
            &prefix_path,
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &install_driver,
            &InstallOptions::default(),
        )
        .await;

        assert_check_files!(
            &target_prefix.path(),
            &["dir/clobber.txt", "__clobbers__/clobber-without-dir/dir"],
        );
    }

    // This used to hit an expect in the clobbering code
    #[tokio::test]
    async fn test_file_and_directory_clobber_with_merging() {
        // Create a transaction
        let repodata_record_without_dir = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-fd-rev-3-0.1.0-h4616a5c_0.conda"),
        );
        let repodata_record_with_dir1 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-fd-rev-2-0.1.0-h4616a5c_0.conda"),
        );
        let repodata_record_with_dir2 = get_repodata_record(
            get_test_data_dir().join("clobber/clobber-fd-rev-1-0.1.0-h4616a5c_0.conda"),
        );

        // execute transaction
        let target_prefix = tempfile::tempdir().unwrap();
        let prefix_path = Prefix::create(target_prefix.path()).unwrap();
        let packages_dir = tempfile::tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        // remove one of the clobbering files
        let transaction = transaction::Transaction::<PrefixRecord, RepoDataRecord> {
            operations: vec![
                TransactionOperation::Install(repodata_record_without_dir),
                TransactionOperation::Install(repodata_record_with_dir1),
                TransactionOperation::Install(repodata_record_with_dir2),
            ],
            python_info: None,
            current_python_info: None,
            platform: Platform::current(),
        };

        let install_driver = InstallDriver::builder().with_prefix_records(&[]).finish();

        execute_transaction(
            transaction,
            &prefix_path,
            &reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new()),
            &cache,
            &install_driver,
            &InstallOptions::default(),
        )
        .await;

        assert_check_files!(
            &target_prefix.path(),
            &[
                "clobber/clobber-fd-rev-1.txt",
                "clobber/clobber-fd-rev-2.txt",
                "__clobbers__/clobber-fd-rev-3/clobber"
            ],
        );
    }
}
