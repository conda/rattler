//! Contains business logic to retrieve the results from libsolv after attempting to resolve a conda
//! environment

use super::{
    wrapper::keys::{SOLVABLE_EXTRA_NAME, SOLVABLE_EXTRA_PACKAGE, SOLVABLE_REPODATA_RECORD_INDEX},
    wrapper::pool::{Pool, StringId},
    wrapper::repo::RepoId,
    wrapper::solvable::SolvableId,
    wrapper::transaction::Transaction,
    wrapper::{ffi, solvable},
};
use rattler_conda_types::{PackageName, RepoDataRecord};
use std::collections::HashMap;

/// Result of extracting packages and extras from the solver transaction.
pub struct SolverOutput {
    pub packages: Vec<RepoDataRecord>,
    pub extras: HashMap<PackageName, Vec<String>>,
}

/// Returns which packages should be installed in the environment and which extras are activated.
///
/// If the transaction contains libsolv operations that are not "install" an error is returned
/// containing their ids.
pub fn get_required_packages(
    pool: &Pool,
    repo_mapping: &HashMap<RepoId, usize>,
    transaction: &Transaction<'_>,
    repodata_records: &[Vec<&RepoDataRecord>],
) -> Result<SolverOutput, Vec<ffi::Id>> {
    let mut required_packages = Vec::new();
    let mut extras: HashMap<PackageName, Vec<String>> = HashMap::new();
    let mut unsupported_operations = Vec::new();

    let solvable_index_id = pool
        .find_interned_str(SOLVABLE_REPODATA_RECORD_INDEX)
        .unwrap();

    // Keys for retrieving extra info from synthetic solvables
    let extra_package_id = pool.find_interned_str(SOLVABLE_EXTRA_PACKAGE);
    let extra_name_id = pool.find_interned_str(SOLVABLE_EXTRA_NAME);

    // Safe because `transaction.steps` is an active queue
    let transaction_queue = transaction.get_steps();

    for id in transaction_queue.iter() {
        let transaction_type = transaction.transaction_type(id);

        // Try to retrieve the repodata record corresponding to this solvable
        if let Some((repo_index, solvable_index)) =
            get_solvable_indexes(pool, repo_mapping, solvable_index_id, id)
        {
            let repodata_record = repodata_records[repo_index][solvable_index];

            match transaction_type as u32 {
                ffi::SOLVER_TRANSACTION_INSTALL => {
                    required_packages.push(repodata_record.clone());
                }
                _ => {
                    unsupported_operations.push(transaction_type);
                }
            }
        } else if transaction_type as u32 == ffi::SOLVER_TRANSACTION_INSTALL {
            // This might be a synthetic solvable for an extra
            // Extract the extra information from stored attributes
            if let Some((pkg_id, name_id)) = extra_package_id.zip(extra_name_id) {
                if let Some((pkg_name, extra_name)) =
                    extract_extra_from_solvable(pool, id, pkg_id, name_id)
                {
                    extras.entry(pkg_name).or_default().push(extra_name);
                }
            }
        }
    }

    if !unsupported_operations.is_empty() {
        return Err(unsupported_operations);
    }

    Ok(SolverOutput {
        packages: required_packages,
        extras,
    })
}

/// Extracts the package name and extra name from a synthetic solvable using stored attributes.
fn extract_extra_from_solvable(
    pool: &Pool,
    id: SolvableId,
    extra_package_key: StringId,
    extra_name_key: StringId,
) -> Option<(PackageName, String)> {
    let solvable = id.resolve_raw(pool);
    let solvable_ptr = solvable.as_ptr();

    let pkg_name_id = solvable::lookup_id(solvable_ptr, extra_package_key)?;
    let extra_name_id = solvable::lookup_id(solvable_ptr, extra_name_key)?;

    let pkg_name_str = pkg_name_id.resolve(pool)?;
    let extra_name_str = extra_name_id.resolve(pool)?;

    let pkg_name = pkg_name_str.parse::<PackageName>().ok()?;
    Some((pkg_name, extra_name_str.to_string()))
}

fn get_solvable_indexes(
    pool: &Pool,
    repo_mapping: &HashMap<RepoId, usize>,
    solvable_index_id: StringId,
    id: SolvableId,
) -> Option<(usize, usize)> {
    let solvable = id.resolve_raw(pool);
    let solvable_index = solvable::lookup_num(solvable.as_ptr(), solvable_index_id)? as usize;

    // Safe because there are no active mutable borrows of any solvable at this stage
    let repo_id = RepoId::from_ffi_solvable(unsafe { solvable.as_ref() });

    let repo_index = repo_mapping[&repo_id];

    Some((repo_index, solvable_index))
}
