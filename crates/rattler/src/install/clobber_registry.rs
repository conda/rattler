//! Implements a registry for "clobbering" files (files that are appearing in multiple packages)

use std::{
    collections::{hash_map::Entry, HashMap},
    path::{Path, PathBuf},
};

use rattler_conda_types::{package::PathsJson, PackageName, PrefixRecord};

/// A registry for clobbering files
/// The registry keeps track of all files that are installed by a package and
/// can be used to rename files that are already installed by another package.
#[derive(Default, Debug)]
pub struct ClobberRegistry {
    paths_registry: HashMap<PathBuf, usize>,
    clobbers: HashMap<PathBuf, Vec<usize>>,
    package_names: Vec<PackageName>,
}

impl ClobberRegistry {
    /// Create a new clobber registry that is initialized with the given prefix records.
    pub fn from_prefix_records(prefix_records: &[PrefixRecord]) -> Self {
        let mut registry = Self::default();

        let mut temp_clobbers = Vec::new();
        for prefix_record in prefix_records {
            registry
                .package_names
                .push(prefix_record.repodata_record.package_record.name.clone());

            let clobber_ending = format!(
                "__clobber-from-{}",
                prefix_record
                    .repodata_record
                    .package_record
                    .name
                    .as_normalized()
            );
            for p in &prefix_record.files {
                println!("Checking path: {}", p.display());
                if p.to_string_lossy().ends_with(&clobber_ending) {
                    // register a clobbered path
                    if let Some((filename, originating_package)) = p
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .split_once("__clobber-from-")
                    {
                        let path = p.with_file_name(filename);
                        temp_clobbers.push((path, originating_package.to_string()));
                    }
                } else {
                    registry
                        .paths_registry
                        .insert(p.clone(), registry.package_names.len() - 1);
                }
            }
        }
        println!("Temp clobbers: {:#?}", temp_clobbers);
        for (path, originating_package) in temp_clobbers.iter() {
            println!("Clobbering path: {}", path.display());
            println!("Clobbers: {:#?}", registry.clobbers);
            let idx = registry
                .package_names
                .iter()
                .position(|n| n.as_normalized() == originating_package)
                .unwrap();

            registry
                .clobbers
                .entry(path.clone())
                .or_insert_with(|| vec![*registry.paths_registry.get(path).unwrap()])
                .push(idx);
        }

        println!("Clobber registry: {registry:#?}");

        registry
    }

    pub fn clobber_name(path: &Path, package_name: &PackageName) -> PathBuf {
        let file_name = path.file_name().unwrap_or_default();
        let mut new_path = path.to_path_buf();
        new_path.set_file_name(format!(
            "{}__clobber-from-{}",
            file_name.to_string_lossy(),
            package_name.as_normalized()
        ));
        new_path
    }

    pub fn register_paths(
        &mut self,
        name: &str,
        paths_json: &PathsJson,
    ) -> HashMap<PathBuf, PathBuf> {
        let mut clobber_paths = HashMap::new();

        // check if we have the package name already registered
        let name_idx = if let Some(idx) = self
            .package_names
            .iter()
            .position(|n| n.as_normalized() == name)
        {
            idx
        } else {
            self.package_names
                .push(PackageName::try_from(name).unwrap());
            self.package_names.len() - 1
        };

        for entry in paths_json.paths.iter() {
            let path = entry.relative_path.clone();

            if let Entry::Vacant(e) = self.paths_registry.entry(path.clone()) {
                e.insert(name_idx);
            } else {
                // rename to
                let mut new_path = path.clone();
                new_path.set_file_name(Self::clobber_name(&path, &self.package_names[name_idx]));

                self.clobbers
                    .entry(path.clone())
                    .or_insert_with(|| vec![*self.paths_registry.get(&path).unwrap()])
                    .push(name_idx);

                clobber_paths.insert(path, new_path);
            }
        }

        clobber_paths
    }

    /// Unclobber the final paths
    pub fn post_process(&self, sorted_prefix_records: &[&PrefixRecord], target_prefix: &Path) {
        let sorted_names = sorted_prefix_records
            .iter()
            .map(|p| p.repodata_record.package_record.name.clone())
            .collect::<Vec<_>>();
        println!("Post processing the clobbers");
        let conda_meta = target_prefix.join("conda-meta");

        for (path, clobbered_by) in self.clobbers.iter() {
            let clobbered_by_names = clobbered_by
                .iter()
                .map(|&idx| self.package_names[idx].clone())
                .collect::<Vec<_>>();

            println!("Clobbered by: {:?}", clobbered_by_names);
            // extract the subset of clobbered_by that is in sorted_prefix_records
            let sorted_clobbered_by = sorted_names
                .iter()
                .cloned()
                .enumerate()
                .filter(|(_, n)| clobbered_by_names.contains(&n))
                .collect::<Vec<_>>();
            let winner = sorted_clobbered_by.last().expect("No winner found");

            println!("Our current winner is: {:?}", winner);
            println!("sorting: {:?}", sorted_clobbered_by);

            if winner.1 == clobbered_by_names[0] {
                println!(
                    "clobbering decision: keep {} from {:?}",
                    path.display(),
                    winner
                );
            } else {
                let full_path = target_prefix.join(path);
                if full_path.exists() {
                    let loser_name = &clobbered_by_names[0];
                    let loser_path = Self::clobber_name(path, loser_name);

                    std::fs::rename(target_prefix.join(path), target_prefix.join(&loser_path))
                        .expect("Could not rename file");

                    let loser_idx = sorted_clobbered_by
                        .iter()
                        .find(|(_, n)| n == loser_name)
                        .unwrap()
                        .0;

                    let loser_prefix_record = rename_path_in_prefix_record(
                        sorted_prefix_records[loser_idx],
                        path,
                        &PathBuf::from(loser_path),
                    );

                    println!(
                        "clobbering decision: remove {} from {:?}",
                        path.display(),
                        loser_name
                    );

                    loser_prefix_record
                        .write_to_path(conda_meta.join(loser_prefix_record.file_name()), true)
                        .expect("Could not write prefix record");
                }

                let winner_path = Self::clobber_name(path, &winner.1);

                println!(
                    "clobbering decision: choose {} from {:?}",
                    path.display(),
                    winner
                );

                std::fs::rename(target_prefix.join(&winner_path), target_prefix.join(path))
                    .expect("Could not rename file");

                let winner_prefix_record = rename_path_in_prefix_record(
                    sorted_prefix_records[winner.0],
                    &winner_path,
                    path,
                );
                winner_prefix_record
                    .write_to_path(conda_meta.join(winner_prefix_record.file_name()), true)
                    .expect("Could not write prefix record");
            }
        }
    }
}

fn rename_path_in_prefix_record(
    record: &PrefixRecord,
    old_path: &Path,
    new_path: &Path,
) -> PrefixRecord {
    let mut new_record = record.clone();
    let mut new_paths = Vec::new();

    let old_path = PathBuf::from(old_path);
    for path in record.files.iter() {
        if path == &old_path {
            new_paths.push(new_path.to_path_buf());
        } else {
            new_paths.push(path.clone());
        }
    }

    let mut new_path_records = Vec::new();
    for path in record.paths_data.paths.iter() {
        if path.relative_path == old_path {
            let mut element = path.clone();
            element.relative_path = new_path.to_path_buf();
            new_path_records.push(element);
        } else {
            new_path_records.push(path.clone());
        }
    }

    new_record.files = new_paths;
    new_record.paths_data.paths = new_path_records;
    new_record
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use futures::TryFutureExt;
    use rand::seq::SliceRandom;
    use rattler_conda_types::{
        package::IndexJson, PackageRecord, Platform, PrefixRecord, RepoDataRecord,
    };
    use rattler_digest::{Md5, Sha256};
    use rattler_networking::{retry_policies::default_retry_policy, AuthenticatedClient};
    use rattler_package_streaming::seek::read_package_file;
    use transaction::{Transaction, TransactionOperation};

    use crate::{
        get_test_data_dir,
        install::{transaction, unlink_package, InstallDriver, InstallOptions},
        package_cache::PackageCache,
    };

    fn repodata_record(filename: &str) -> RepoDataRecord {
        let path = fs::canonicalize(get_test_data_dir().join(filename)).unwrap();
        let index_json = read_package_file::<IndexJson>(&path).unwrap();

        // find size and hash
        let size = fs::metadata(&path).unwrap().len();
        let sha256 = rattler_digest::compute_file_digest::<Sha256>(&path).unwrap();
        let md5 = rattler_digest::compute_file_digest::<Md5>(&path).unwrap();

        RepoDataRecord {
            package_record: PackageRecord::from_index_json(
                index_json,
                Some(size),
                Some(sha256),
                Some(md5),
            )
            .unwrap(),
            file_name: filename.to_string(),
            url: url::Url::from_file_path(&path).unwrap(),
            channel: "clobber".to_string(),
        }
    }

    /// Install a package into the environment and write a `conda-meta` file that contains information
    /// about how the file was linked.
    async fn install_package_to_environment(
        target_prefix: &Path,
        package_dir: PathBuf,
        repodata_record: RepoDataRecord,
        install_driver: &InstallDriver,
        install_options: &InstallOptions,
    ) -> anyhow::Result<()> {
        // Link the contents of the package into our environment. This returns all the paths that were
        // linked.
        let paths = crate::install::link_package(
            &package_dir,
            target_prefix,
            install_driver,
            install_options.clone(),
        )
        .await?;

        // Construct a PrefixRecord for the package
        let prefix_record = PrefixRecord {
            repodata_record,
            package_tarball_full_path: None,
            extracted_package_dir: Some(package_dir),
            files: paths
                .iter()
                .map(|entry| entry.relative_path.clone())
                .collect(),
            paths_data: paths.into(),
            // TODO: Retrieve the requested spec for this package from the request
            requested_spec: None,
            // TODO: What to do with this?
            link: None,
        };

        // Create the conda-meta directory if it doesnt exist yet.
        let target_prefix = target_prefix.to_path_buf();
        match tokio::task::spawn_blocking(move || {
            let conda_meta_path = target_prefix.join("conda-meta");
            std::fs::create_dir_all(&conda_meta_path)?;

            // Write the conda-meta information
            let pkg_meta_path = conda_meta_path.join(prefix_record.file_name());
            prefix_record.write_to_path(pkg_meta_path, true)
        })
        .await
        {
            Ok(result) => Ok(result?),
            Err(err) => {
                if let Ok(panic) = err.try_into_panic() {
                    std::panic::resume_unwind(panic);
                }
                // The operation has been cancelled, so we can also just ignore everything.
                Ok(())
            }
        }
    }

    async fn execute_operation(
        target_prefix: &Path,
        download_client: &AuthenticatedClient,
        package_cache: &PackageCache,
        install_driver: &InstallDriver,
        op: TransactionOperation<PrefixRecord, RepoDataRecord>,
        install_options: &InstallOptions,
    ) {
        // Determine the package to install
        let install_record = op.record_to_install();
        let remove_record = op.record_to_remove();

        if let Some(remove_record) = remove_record {
            unlink_package(target_prefix, remove_record).await.unwrap();
        }

        let install_package = if let Some(install_record) = install_record {
            // Make sure the package is available in the package cache.
            package_cache
                .get_or_fetch_from_url_with_retry(
                    &install_record.package_record,
                    install_record.url.clone(),
                    download_client.clone(),
                    default_retry_policy(),
                )
                .map_ok(|cache_dir| Some((install_record.clone(), cache_dir)))
                .map_err(anyhow::Error::from)
                .await
                .unwrap()
        } else {
            None
        };

        // If there is a package to install, do that now.
        if let Some((record, package_dir)) = install_package {
            install_package_to_environment(
                target_prefix,
                package_dir,
                record.clone(),
                install_driver,
                install_options,
            )
            .await
            .unwrap();
        }
    }

    async fn execute_transaction(
        transaction: Transaction<PrefixRecord, RepoDataRecord>,
        target_prefix: &Path,
        download_client: &AuthenticatedClient,
        package_cache: &PackageCache,
        install_driver: &InstallDriver,
        install_options: &InstallOptions,
    ) {
        for op in transaction.operations {
            execute_operation(
                target_prefix,
                download_client,
                package_cache,
                install_driver,
                op,
                install_options,
            )
            .await;
        }

        let prefix_records = PrefixRecord::collect_from_prefix(target_prefix).unwrap();
        install_driver
            .post_process(&prefix_records, target_prefix)
            .unwrap();
    }

    fn find_prefix_record<'a>(
        prefix_records: &'a [PrefixRecord],
        name: &str,
    ) -> Option<&'a PrefixRecord> {
        prefix_records
            .iter()
            .find(|r| r.repodata_record.package_record.name.as_normalized() == name)
    }

    fn test_operations() -> Vec<TransactionOperation<PrefixRecord, RepoDataRecord>> {
        let repodata_record_1 = repodata_record("clobber/clobber-1-0.1.0-h4616a5c_0.tar.bz2");
        let repodata_record_2 = repodata_record("clobber/clobber-2-0.1.0-h4616a5c_0.tar.bz2");
        let repodata_record_3 = repodata_record("clobber/clobber-3-0.1.0-h4616a5c_0.tar.bz2");

        vec![
            TransactionOperation::Install(repodata_record_1),
            TransactionOperation::Install(repodata_record_2),
            TransactionOperation::Install(repodata_record_3),
        ]
    }

    fn assert_check_files(target_prefix: &Path, expected_files: &[&str]) {
        let files = std::fs::read_dir(target_prefix).unwrap();
        let files = files
            .filter_map(|f| {
                let fx = f.unwrap();
                if fx.file_type().unwrap().is_file() {
                    Some(fx.path())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        assert_eq!(files.len(), expected_files.len());
        for file in files {
            assert!(expected_files.contains(&file.file_name().unwrap().to_string_lossy().as_ref()));
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
            &AuthenticatedClient::default(),
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
            ],
        );

        let prefix_records = PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();

        let prefix_record_clobber_1 = find_prefix_record(&prefix_records, "clobber-1").unwrap();
        assert!(prefix_record_clobber_1.files == vec![PathBuf::from("clobber.txt")]);
        assert_eq!(prefix_record_clobber_1.paths_data.paths.len(), 1);
        assert_eq!(
            prefix_record_clobber_1.paths_data.paths[0].relative_path,
            PathBuf::from("clobber.txt")
        );
        let prefix_record_clobber_2 = find_prefix_record(&prefix_records, "clobber-2").unwrap();
        assert!(
            prefix_record_clobber_2.files
                == vec![PathBuf::from("clobber.txt__clobber-from-clobber-2")]
        );
        assert_eq!(prefix_record_clobber_2.paths_data.paths.len(), 1);
        assert_eq!(
            prefix_record_clobber_2.paths_data.paths[0].relative_path,
            PathBuf::from("clobber.txt__clobber-from-clobber-2")
        );

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

        let install_driver = InstallDriver::new(100, Some(&prefix_records));

        execute_transaction(
            transaction,
            target_prefix.path(),
            &AuthenticatedClient::default(),
            &cache,
            &install_driver,
            &InstallOptions::default(),
        )
        .await;

        assert_check_files(
            target_prefix.path(),
            &["clobber.txt__clobber-from-clobber-3", "clobber.txt"],
        );

        let prefix_records = PrefixRecord::collect_from_prefix(target_prefix.path()).unwrap();

        let prefix_record_clobber_1 = find_prefix_record(&prefix_records, "clobber-1");
        assert!(prefix_record_clobber_1.is_none());
        let prefix_record_clobber_2 = find_prefix_record(&prefix_records, "clobber-2").unwrap();
        assert!(prefix_record_clobber_2.files == vec![PathBuf::from("clobber.txt")]);
        assert_eq!(prefix_record_clobber_2.paths_data.paths.len(), 1);
        assert_eq!(
            prefix_record_clobber_2.paths_data.paths[0].relative_path,
            PathBuf::from("clobber.txt")
        );

        assert_eq!(
            fs::read_to_string(target_prefix.path().join("clobber.txt")).unwrap(),
            "clobber-2\n"
        );
        let prefix_record_clobber_3 = find_prefix_record(&prefix_records, "clobber-3").unwrap();
        assert!(
            prefix_record_clobber_3.files
                == vec![PathBuf::from("clobber.txt__clobber-from-clobber-3")]
        );

        assert_eq!(prefix_record_clobber_3.paths_data.paths.len(), 1);

        assert_eq!(
            prefix_record_clobber_3.paths_data.paths[0].relative_path,
            PathBuf::from("clobber.txt__clobber-from-clobber-3")
        );
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
                &AuthenticatedClient::default(),
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
                ],
            );
        }
    }
}
