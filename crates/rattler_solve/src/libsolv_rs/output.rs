//! Contains business logic to retrieve the results from libsolv after attempting to resolve a conda
//! environment

use rattler_libsolv_rs::{Pool, RepoId, SolvableId, Transaction};
use rattler_conda_types::RepoDataRecord;
use std::collections::HashMap;

/// Returns which packages should be installed in the environment
///
/// If the transaction contains libsolv operations that are not "install" an error is returned
/// containing their ids.
pub fn get_required_packages(
    pool: &Pool,
    repo_mapping: &HashMap<RepoId, usize>,
    transaction: &Transaction,
    repodata_records: &[Vec<&RepoDataRecord>],
) -> Vec<RepoDataRecord> {
    let mut required_packages = Vec::new();

    for &id in &transaction.steps {
        // Retrieve the repodata record corresponding to this solvable
        //
        // Packages without indexes are virtual and can be ignored
        if let Some((repo_index, solvable_index)) = get_solvable_indexes(pool, repo_mapping, id) {
            let repodata_record = repodata_records[repo_index][solvable_index];
            required_packages.push(repodata_record.clone());
        }
    }

    required_packages
}

fn get_solvable_indexes(
    pool: &Pool,
    repo_mapping: &HashMap<RepoId, usize>,
    id: SolvableId,
) -> Option<(usize, usize)> {
    let solvable = pool.resolve_solvable(id);
    let solvable_index = solvable.metadata.original_index?;

    let repo_id = solvable.repo_id();
    let repo_index = repo_mapping[&repo_id];

    Some((repo_index, solvable_index))
}
