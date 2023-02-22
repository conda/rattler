use super::ffi;
use super::pool::{Pool, StringId};
use super::repo::RepoId;
use super::solvable::{self, SolvableId};
use super::solver::Solver;
use crate::libsolv::wrapper::queue::QueueRef;
use crate::package_operation::{PackageOperation, PackageOperationKind};
use rattler_conda_types::RepoDataRecord;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ptr::NonNull;

/// Wrapper for [`ffi::Transaction`], which is an abstraction over changes that need to be
/// done to satisfy the dependency constraints
///
/// The wrapper functions as an owned pointer, guaranteed to be non-null and freed
/// when the Transaction is dropped
pub struct Transaction<'solver>(
    NonNull<ffi::Transaction>,
    PhantomData<&'solver Solver<'solver>>,
);

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        // Safe because we know that the transaction is never freed manually
        unsafe { ffi::transaction_free(self.0.as_mut()) }
    }
}

impl Transaction<'_> {
    /// Constructs a new Transaction from the provided libsolv pointer. It is the responsibility of the
    /// caller to ensure the pointer is actually valid.
    pub(super) unsafe fn new<'a>(
        _solver: &'a Solver<'a>,
        ptr: NonNull<ffi::Transaction>,
    ) -> Transaction<'a> {
        Transaction(ptr, PhantomData::default())
    }

    /// Returns a raw pointer to the wrapped `ffi::Transaction`, to be used for calling ffi functions
    /// that require access to the repodata (and for nothing else)
    fn raw_ptr(&self) -> *mut ffi::Transaction {
        self.0.as_ptr()
    }

    /// Returns a reference to the wrapped `ffi::Transaction`.
    fn as_ref(&self) -> &ffi::Transaction {
        unsafe { self.0.as_ref() }
    }

    /// Returns the transaction type
    fn transaction_type(&self, solvable_id: SolvableId) -> ffi::Id {
        unsafe {
            ffi::transaction_type(
                self.raw_ptr(),
                solvable_id.into(),
                ffi::SOLVER_TRANSACTION_SHOW_ALL as std::os::raw::c_int,
            )
        }
    }

    /// Returns the package operations derived from the transaction
    ///
    /// If the transaction contains libsolv operations that have no mapping to `PackageOperation`,
    /// an error is returned containing their ids
    pub fn get_package_operations(
        &mut self,
        pool: &Pool,
        repo_mapping: &HashMap<RepoId, usize>,
        repodata_records: &[&[RepoDataRecord]],
    ) -> Result<Vec<PackageOperation>, Vec<ffi::Id>> {
        let mut package_operations = Vec::new();
        let mut unsupported_operations = Vec::new();

        let solvable_index_id = pool
            .find_interned_str("solvable:repodata_record_index")
            .unwrap();

        let transaction = self.as_ref();

        // Safe because `transaction.steps` is an active queue
        let transaction_queue = unsafe { QueueRef::from_ffi_queue(transaction, transaction.steps) };

        for id in transaction_queue.iter() {
            let id = SolvableId(id);
            let transaction_type = self.transaction_type(id);

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

                    // Safe because the provided id is valid
                    let solvable_offset =
                        unsafe { ffi::transaction_obs_pkg(self.raw_ptr(), id.into()) };

                    let second_solvable_id = SolvableId(solvable_offset);
                    let (second_repo_index, second_solvable_index) = get_solvable_indexes(
                        pool,
                        repo_mapping,
                        solvable_index_id,
                        second_solvable_id,
                    );
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
