use super::ffi;
use super::pool::{Pool, StringId};
use super::repo::RepoId;
use super::solvable::{self, SolvableId};
use crate::package_operation::{PackageOperation, PackageOperationKind};
use rattler_conda_types::RepoDataRecord;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

/// Wraps a pointer to an `ffi::Transaction` which is freed when the instance is dropped.
#[repr(transparent)]
struct TransactionOwnedPtr(NonNull<ffi::Transaction>);

impl Drop for TransactionOwnedPtr {
    fn drop(&mut self) {
        // Safe because the pointer must not be null
        unsafe { ffi::transaction_free(self.0.as_mut()) }
    }
}

/// This represents a transaction in libsolv which is a abstraction over changes that need to be
/// done to satisfy the dependency constraint.
pub struct Transaction<'solver>(TransactionOwnedPtr, PhantomData<&'solver ffi::Transaction>);

impl Transaction<'_> {
    /// Constructs a new instance
    pub(super) fn new(ptr: NonNull<ffi::Transaction>) -> Self {
        Transaction(TransactionOwnedPtr(ptr), PhantomData::default())
    }
}

/// A `TransactionRef` is a wrapper around an `ffi::Transaction` that provides a safe abstraction
/// over its functionality.
///
/// A `TransactionRef` can not be constructed by itself but is instead returned by dereferencing a
/// [`Transaction`].
#[repr(transparent)]
pub struct TransactionRef(ffi::Transaction);

impl Deref for Transaction<'_> {
    type Target = TransactionRef;

    fn deref(&self) -> &Self::Target {
        unsafe { self.0 .0.cast().as_ref() }
    }
}

impl DerefMut for Transaction<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.0 .0.cast().as_mut() }
    }
}

impl TransactionRef {
    /// Returns a pointer to the wrapped `ffi::Transaction`
    fn as_ptr(&self) -> NonNull<ffi::Transaction> {
        // Safe because a `TransactionRef` is a transparent wrapper around `ffi::Transaction`
        unsafe { NonNull::new_unchecked(self as *const Self as *mut Self).cast() }
    }

    /// Returns a reference to the wrapped `ffi::Transaction`.
    fn as_ref(&self) -> &ffi::Transaction {
        // Safe because a `TransactionRef` is a transparent wrapper around `ffi::Transaction`
        unsafe { std::mem::transmute(self) }
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

        // Get inner transaction type
        let inner = self.as_ref();
        // Number of transaction details
        let count = inner.steps.count as usize;

        let solvable_index_id = pool
            .find_intern_str("solvable:repodata_record_index")
            .unwrap();

        // TODO: simplify unsafe usage and explain why it is all right
        for index in 0..count {
            unsafe {
                // Get the id for the current solvable
                // Safe because we don't go past the count
                let raw_id = *inner.steps.elements.add(index);
                let id = SolvableId(raw_id);
                // Get the transaction type
                let id_type = ffi::transaction_type(
                    self.as_ptr().as_ptr(),
                    id.into(),
                    ffi::SOLVER_TRANSACTION_SHOW_ALL as std::os::raw::c_int,
                );

                let (repo_index, solvable_index) =
                    get_solvable_indexes(pool, repo_mapping, solvable_index_id, id);
                let repodata_record = &repodata_records[repo_index][solvable_index];

                match id_type as u32 {
                    ffi::SOLVER_TRANSACTION_DOWNGRADED
                    | ffi::SOLVER_TRANSACTION_UPGRADED
                    | ffi::SOLVER_TRANSACTION_CHANGED => {
                        package_operations.push(PackageOperation {
                            package: repodata_record.clone(),
                            kind: PackageOperationKind::Remove,
                        });

                        let solvable_offset =
                            ffi::transaction_obs_pkg(self.as_ptr().as_ptr(), raw_id);
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
                        unsupported_operations.push(id_type);
                    }
                }
            };
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
    let solvable = id.resolve(pool);
    let solvable_index = solvable::lookup_num(solvable, solvable_index_id).unwrap() as usize;
    let repo_id = RepoId::from_ffi_solvable(solvable);
    let repo_index = repo_mapping[&repo_id];

    (repo_index, solvable_index)
}
