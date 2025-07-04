//! Implements a registry for "clobbering" files (files that are appearing in
//! multiple packages)

use std::process::{Command, Stdio};
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

fn tree(path: &Path) -> String {
    let output = Command::new("tree")
        .arg(path)
        .stdout(Stdio::piped())
        .output()
        .unwrap();

    String::from_utf8(output.stdout).unwrap()
}

const CLOBBERS_DIR_NAME: &str = "__clobbers__";

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
    #[error("Unclobbering was called second time and this is currently unsupported")]
    AlreadyUnclobbered,
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

    /// Current implementation doesn't allow to call unclobbering with
    /// desired behaviour, so we track if we already did unclobbering
    /// to report error on second time.
    did_unclobbering: bool,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
struct PackageNameIdx(usize);

impl ClobberRegistry {
    /// Create a new clobber registry that is initialized with the given prefix
    /// records.
    pub fn new<'i>(prefix_records: impl IntoIterator<Item = &'i PrefixRecord>) -> Self {
        let mut registry = ClobberRegistry::default();

        let mut prefix_records = prefix_records.into_iter().collect::<Vec<_>>();
        let previous_winner_idx = prefix_records.iter().position(|&pr| {
            pr.paths_data
                .paths
                .iter()
                .all(|pe| pe.original_path.is_none())
        });

        // I think we assume here that every conflict will occur
        // independently of order in which we register prefix records
        // if first record is previous winner or smth.
        //
        // We should test that
        if let Some(previous_winner_idx) = previous_winner_idx {
            if prefix_records.len() > 1 && previous_winner_idx != 0 {
                prefix_records.swap(0, previous_winner_idx);
            }
        }

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
        eprintln!("-----Unregistering {}", normalized_name);

        self.path_resolver.remove_package(normalized_name);
        // // Since we're need to report new conflicts only in
        // // `register_paths` we can simply copy current state of
        // // conflicts from the `path_resolver` as its `remove_package`
        // // method will recalculate conflicts in package removal if
        // // some conflict existed before.
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

        // Packages with higher indices have a priority, but we want
        // first package to have highest priority, so we have to
        // reverse indices.
        let inverted_package_idx = usize::MAX - package_idx.0;

        let intermediate_conflicts =
            self.path_resolver
                .add_package(normalized_name, inverted_package_idx, &paths);

        // We want to update conflicts in path resovler to obtain
        // difference for correct link paths.
        self.path_resolver.resolve_conflicts();

        // Map that will be used to link file to appropriate place to
        // avoid writing two different files to the same place.
        let link_paths = intermediate_conflicts
            .into_iter()
            .map(|p| {
                (
                    p.clone(),
                    get_clobber_relative_path(&Entry {
                        path: p,
                        entry_type: EntryType::File, // doesn't matter
                        package_index: 0,
                        package_name: normalized_name.to_string(),
                    }),
                )
            })
            .collect();

        self.sync_conflicts();

        eprintln!("+++++Registering {}", normalized_name);
        dbg!(link_paths)
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
    ///
    /// There are several steps that we are doing:
    /// 1. Reprioritize based on the given sorted_prefix_records to get correct paths in the final layout.
    /// 2. Resolve paths based on the path resolver conflicts.
    /// 3. Swap current owner (first one who took ownership) of path with winner.
    /// 4. Compute result of unclobbering.
    /// 5. Write conda-meta file.
    /// 6. Return result of unclobbering.
    pub fn unclobber(
        &mut self,
        sorted_prefix_records: &[&PrefixRecord],
        target_prefix: &Path,
    ) -> Result<HashMap<PathBuf, ClobberedPath>, ClobberError> {
        if self.did_unclobbering {
            return Err(ClobberError::AlreadyUnclobbered);
        }
        self.did_unclobbering = true;

        // eprintln!("\\\\\\\\\\Before\n{}", tree(target_prefix));
        // Needed for step 5.
        // We store copy of prefix records, so we can update them later on.
        // They will be used to update metadata.
        let mut prefix_records: Vec<PrefixRecord> =
            sorted_prefix_records.iter().map(|&pr| pr.clone()).collect();
        let mut prefix_records_to_rewrite: HashSet<usize> = HashSet::new();

        // 1
        let new_priorities = sorted_prefix_records
            .iter()
            .enumerate()
            .map(|(priority, &pr)| {
                let name = pr.name().as_normalized().to_string();
                (name, priority)
            })
            .collect();
        self.path_resolver.reprioritize(&new_priorities);

        // 2
        self.path_resolver.resolve_conflicts();

        // 3
        let resolved_conflicts = self.path_resolver.get_conflicts();
        self.swap_clobbered_files(
            target_prefix,
            resolved_conflicts,
            &mut prefix_records,
            &mut prefix_records_to_rewrite,
        )?;

        // 4
        let result = resolved_conflicts
            .iter()
            .map(|conflict| self.conflict_to_clobbered_path(conflict))
            .collect();

        // 5
        self.update_conda_meta(target_prefix, &prefix_records, &prefix_records_to_rewrite)?;

        eprintln!("/////After\n{}", tree(target_prefix));

        // 6
        Ok(result)
    }

    /// Swap files on-disk to match current winners. Basically
    /// syncronizes in-memory resolved conflict with what is on the
    /// file system.
    ///
    /// It not only moves files, but also updates in-memory prefix records.
    fn swap_clobbered_files(
        &self,
        target_prefix: &Path,
        new_conflicts: &[Conflict],
        prefix_records: &mut [PrefixRecord],
        prefix_records_to_rewrite: &mut HashSet<usize>,
    ) -> Result<(), ClobberError> {
        let old_conflicts = self.conflicts.as_slice();

        let (updated_conflicts, only_old_conflicts, only_new_conflicts) =
            join_and_partition_conflicts(old_conflicts, new_conflicts);

        // Since we only reprioritized packages we should have only matching conflicts
        assert!(only_old_conflicts.is_empty());
        assert!(only_new_conflicts.is_empty());

        // This one particular edge case mean that we have to copy all clobbered package files to prefix as there is no conflicts.
        if updated_conflicts.is_empty() {
            let final_layout = self.path_resolver.get_final_layout();
            for entry in final_layout.values() {
                let new_winner = entry;
                let new_winner_idx = new_winner.package_index;
                if self.is_in_clobbers(target_prefix, new_winner) {
                    let (from, to, new_path_is_clobbered) =
                        self.toggle_clobber_entry_on_disk(target_prefix, new_winner)?;
                    rename_path_in_prefix_record(
                        &mut prefix_records[new_winner_idx],
                        &from,
                        &to,
                        new_path_is_clobbered,
                    );

                    prefix_records_to_rewrite.insert(new_winner_idx);
                }
            }
        }

        // Assume that we first have direct conflicts, and only then blocked
        for (old, new) in updated_conflicts {
            let previous_winner = &dbg!(&old.winner);
            let new_winner = &dbg!(&new.winner);

            let previous_winner_in_clobbers = target_prefix
                .join(get_clobber_relative_path(previous_winner))
                .exists();
            let new_winner_in_clobbers = target_prefix
                .join(get_clobber_relative_path(new_winner))
                .exists();
            let both_in_clobbers = previous_winner_in_clobbers && new_winner_in_clobbers;

            if &previous_winner.package_name != &new_winner.package_name {
                if !both_in_clobbers {
                    let previous_winner_idx = new
                        .losers
                        .iter()
                        .find(|e| e.package_name == previous_winner.package_name)
                        .unwrap()
                        .package_index;

                    let (from, to, new_path_is_clobbered) =
                        self.toggle_clobber_entry_on_disk(target_prefix, previous_winner)?;
                    rename_path_in_prefix_record(
                        &mut prefix_records[previous_winner_idx],
                        &from,
                        &to,
                        new_path_is_clobbered,
                    );
                    prefix_records_to_rewrite.insert(previous_winner_idx);
                }

                let new_winner_idx = new_winner.package_index;
                let (from, to, new_path_is_clobbered) =
                    self.toggle_clobber_entry_on_disk(target_prefix, new_winner)?;
                rename_path_in_prefix_record(
                    &mut prefix_records[new_winner_idx],
                    &from,
                    &to,
                    new_path_is_clobbered,
                );

                prefix_records_to_rewrite.insert(new_winner_idx);
            } else if new_winner_in_clobbers {
                let new_winner_idx = new_winner.package_index;
                let (from, to, new_path_is_clobbered) =
                    self.toggle_clobber_entry_on_disk(target_prefix, new_winner)?;
                rename_path_in_prefix_record(
                    &mut prefix_records[new_winner_idx],
                    &from,
                    &to,
                    new_path_is_clobbered,
                );

                prefix_records_to_rewrite.insert(new_winner_idx);
            }
        }

        Ok(())
    }

    /// Move entry to corresponding clobber directory if it is in
    /// `target_prefix`, or move entry from corresponding clobber
    /// directory, if it is not in `target_prefix`.
    fn toggle_clobber_entry_on_disk(
        &self,
        target_prefix: &Path,
        entry: &Entry,
    ) -> Result<(PathBuf, PathBuf, bool), ClobberError> {
        // eprintln!("===================Toggle start\n{}", tree(target_prefix));

        // Assume that file is in target_prefix, if it is not in clobbers, and vice versa.
        let clobber_relative_path = get_clobber_relative_path(entry);
        let clobber_path = clobber_relative_path.as_path();
        let prefix_path = entry.path.as_path();

        let path_is_clobbered = self.is_in_clobbers(target_prefix, entry);

        let (from, to) = if path_is_clobbered {
            (clobber_path, prefix_path)
        } else {
            (prefix_path, clobber_path)
        };

        // eprintln!("Moving from {} to {}", from.display(), to.display());
        // eprintln!("But before, let's ensure that all necessary directories are created.");
        if let Some(parent) = target_prefix.join(to).parent() {
            fs::create_dir_all(parent).map_err(|err| {
                let error_message = format!(
                    "Couldn't create parent directories for path {}",
                    to.display()
                );
                ClobberError::IoError(error_message, err)
            })?;
        }

        fs::rename(target_prefix.join(from), target_prefix.join(to)).map_err(|err| {
            let error_message = format!(
                "Can not move from {} to {}, probably because it is not in target prefix {}",
                from.display(),
                to.display(),
                target_prefix.display()
            );
            ClobberError::IoError(error_message, err)
        })?;

        eprintln!("===================Toggle end\n{}", tree(target_prefix));

        Ok((from.to_path_buf(), to.to_path_buf(), !path_is_clobbered))
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
        prefix_records: &[PrefixRecord],
        prefix_records_to_rewrite: &HashSet<usize>,
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

    /// Check if entry path is currently stored in clobbers directory.
    fn is_in_clobbers(&self, target_prefix: &Path, entry: &Entry) -> bool {
        let clobber_relative_path = get_clobber_relative_path(entry);
        target_prefix.join(clobber_relative_path).exists()
    }
}

/// Constructs a relative path to clobber.
fn get_clobber_relative_path(entry: &Entry) -> PathBuf {
    let package_name = entry.package_name.as_str();
    let relative_file_path = entry.path.as_path();
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
    // dbg!((
    //     record.name().as_normalized(),
    //     old_path,
    //     new_path,
    //     new_path_is_clobber
    // ));
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

/// Groups conflicts pairwise based on their paths. Returns not only pairs, but also conflicts that are only either old or new.
fn join_and_partition_conflicts<'a, 'b>(
    old_conflicts: &'a [Conflict],
    new_conflicts: &'b [Conflict],
) -> (
    Vec<(&'a Conflict, &'b Conflict)>,
    Vec<&'a Conflict>,
    Vec<&'b Conflict>,
) {
    let mut new_conflicts_map: HashMap<_, _> = new_conflicts
        .into_iter()
        .map(|item| (item.path.clone(), item))
        .collect();

    let mut matches = Vec::new();
    let mut only_old_conflicts = Vec::new();

    for old_conflict in old_conflicts {
        if let Some(new_conflict) = new_conflicts_map.remove(&old_conflict.path) {
            matches.push((old_conflict, new_conflict));
        } else {
            only_old_conflicts.push(old_conflict);
        }
    }

    let only_new_conflicts = new_conflicts_map.into_values().collect();

    (matches, only_old_conflicts, only_new_conflicts)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        ffi::OsStr,
        fs,
        path::{Path, PathBuf},
        str::FromStr,
    };

    use insta::assert_yaml_snapshot;
    use itertools::Itertools;
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

    fn collect_paths_to_files(target_prefix: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();

        for entry in walkdir::WalkDir::new(target_prefix)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();

            if path
                .components()
                .map(|c| c.as_os_str())
                .filter(|&c| c == OsStr::new("conda-meta"))
                .next()
                .is_some()
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
        for operations in test_operations.into_iter().permutations(len) {
            let operations = operations.clone();
            // randomize the order of the operations

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

        // dbg!(prefix_records[2].name().as_normalized());
        // dbg!(update_ops[2].package_record.name.as_normalized());

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
    async fn test_directory_clobber() {
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
}
