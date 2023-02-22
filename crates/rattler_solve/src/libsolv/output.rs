//! Contains business logic to retrieve the results from libsolv after attempting to resolve a conda
//! environment

use crate::libsolv::wrapper::pool::{Pool, StringId};
use crate::libsolv::wrapper::repo::RepoId;
use crate::libsolv::wrapper::solvable::SolvableId;
use crate::libsolv::wrapper::transaction::Transaction;
use crate::libsolv::wrapper::{ffi, solvable};
use crate::{PackageOperation, PackageOperationKind};
use rattler_conda_types::RepoDataRecord;
use std::collections::HashMap;

/// Returns the package operations derived from the transaction
///
/// If the transaction contains libsolv operations that have no mapping to `PackageOperation`,
/// an error is returned containing their ids
pub fn get_package_operations(
    pool: &Pool,
    repo_mapping: &HashMap<RepoId, usize>,
    transaction: &Transaction,
    repodata_records: &[&[RepoDataRecord]],
) -> Result<Vec<PackageOperation>, Vec<ffi::Id>> {
    let mut package_operations = Vec::new();
    let mut unsupported_operations = Vec::new();

    let solvable_index_id = pool
        .find_interned_str("solvable:repodata_record_index")
        .unwrap();

    // Safe because `transaction.steps` is an active queue
    let transaction_queue = transaction.get_steps();

    for id in transaction_queue.iter() {
        let transaction_type = transaction.transaction_type(id);

        // Retrieve the repodata record corresponding to this solvable
        let (repo_index, solvable_index) =
            get_solvable_indexes(pool, repo_mapping, solvable_index_id, id);
        let repodata_record = &repodata_records[repo_index][solvable_index];

        match transaction_type as u32 {
            ffi::SOLVER_TRANSACTION_DOWNGRADED
            | ffi::SOLVER_TRANSACTION_UPGRADED
            | ffi::SOLVER_TRANSACTION_CHANGED => {
                package_operations.push(PackageOperation {
                    package: repodata_record.clone(),
                    kind: PackageOperationKind::Remove,
                });

                // Safe because the provided id is valid and the transaction type has a second
                // associated solvable
                let second_solvable_id = unsafe { transaction.obs_pkg(id) };
                let (second_repo_index, second_solvable_index) =
                    get_solvable_indexes(pool, repo_mapping, solvable_index_id, second_solvable_id);
                let second_repodata_record =
                    &repodata_records[second_repo_index][second_solvable_index];

                package_operations.push(PackageOperation {
                    package: second_repodata_record.clone(),
                    kind: PackageOperationKind::Install,
                });
            }
            ffi::SOLVER_TRANSACTION_REINSTALLED => {
                package_operations.push(PackageOperation {
                    package: repodata_record.clone(),
                    kind: PackageOperationKind::Reinstall,
                });
            }
            ffi::SOLVER_TRANSACTION_INSTALL => {
                package_operations.push(PackageOperation {
                    package: repodata_record.clone(),
                    kind: PackageOperationKind::Install,
                });
            }
            ffi::SOLVER_TRANSACTION_ERASE => {
                package_operations.push(PackageOperation {
                    package: repodata_record.clone(),
                    kind: PackageOperationKind::Remove,
                });
            }
            ffi::SOLVER_TRANSACTION_IGNORE => {}
            _ => {
                unsupported_operations.push(transaction_type);
            }
        }
    }

    if !unsupported_operations.is_empty() {
        return Err(unsupported_operations);
    }

    Ok(package_operations)
}

fn get_solvable_indexes(
    pool: &Pool,
    repo_mapping: &HashMap<RepoId, usize>,
    solvable_index_id: StringId,
    id: SolvableId,
) -> (usize, usize) {
    let solvable = id.resolve_raw(pool);
    let solvable_index =
        solvable::lookup_num(solvable.as_ptr(), solvable_index_id).unwrap() as usize;

    // Safe because there are no active mutable borrows of any solvable at this stage
    let repo_id = RepoId::from_ffi_solvable(unsafe { solvable.as_ref() });

    let repo_index = repo_mapping[&repo_id];

    (repo_index, solvable_index)
}
