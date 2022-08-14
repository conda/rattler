use crate::libsolv::ffi;
use crate::libsolv::pool::Pool;
use crate::libsolv::solvable::{Solvable, SolvableId};
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ptr::NonNull;

/// Wrapper type so we do not use lifetime in the drop
#[repr(transparent)]
pub(super) struct TransactionOwnedPtr(NonNull<ffi::Transaction>);

impl Drop for TransactionOwnedPtr {
    fn drop(&mut self) {
        // Safe because the pointer must not be null
        unsafe { ffi::transaction_free(self.0.as_mut()) }
    }
}
impl TransactionOwnedPtr {
    pub fn new(transaction: *mut ffi::Transaction) -> Self {
        Self(NonNull::new(transaction).expect("could not create transaction object"))
    }
}

/// This represents a transaction in libsolv
/// which is a abstraction over changes that need to be done
/// to satisfy the dependency constraint
pub struct Transaction<'solver>(
    pub(super) TransactionOwnedPtr,
    pub(super) PhantomData<&'solver ffi::Solver>,
);

impl<'solver> Transaction<'solver> {
    /// Get the inner pointer as a raw mutable pointer
    fn as_raw_mut(&mut self) -> *mut ffi::Transaction {
        self.0 .0.as_ptr()
    }

    /// Get the inner pointer as a raw pointer
    fn as_raw(&self) -> *const ffi::Transaction {
        self.0 .0.as_ptr()
    }

    /// Access the pool
    fn pool(&self) -> ManuallyDrop<Pool> {
        let pool = (unsafe { *self.as_raw() }).pool;
        ManuallyDrop::new(unsafe { std::mem::transmute::<*mut ffi::Pool, Pool>(pool) })
    }
}

enum InstallOperation {
    Install,
    Reinstall,
    Remove,
    Ignore,
    Unknown,
}

impl From<u32> for InstallOperation {
    fn from(data: u32) -> Self {
        match data {
            ffi::SOLVER_TRANSACTION_DOWNGRADED
            | ffi::SOLVER_TRANSACTION_UPGRADED
            | ffi::SOLVER_TRANSACTION_CHANGED => InstallOperation::Install,
            ffi::SOLVER_TRANSACTION_REINSTALL => InstallOperation::Reinstall,
            ffi::SOLVER_TRANSACTION_ERASE => InstallOperation::Remove,
            ffi::SOLVER_TRANSACTION_IGNORE => InstallOperation::Ignore,
            _ => InstallOperation::Unknown,
        }
    }
}

pub struct OperationOnSolvable {
    solvable: Solvable,
    operation: InstallOperation,
}

impl Transaction<'_> {
    /// Return the solvable operations
    pub fn get_solvable_operations(&mut self) -> Vec<OperationOnSolvable> {
        let mut solvable_operations = Vec::default();
        // Get inner transaction type
        let inner = unsafe { *self.as_raw_mut() };
        // Number of transaction details
        let count = inner.steps.count as usize;
        let pool = self.pool();
        for index in 0..count {
            let (solvable, operation) = unsafe {
                // Get the id for the current solvable
                // Safe because we don't go past the count
                let id = *inner.steps.elements.add(index);
                let id = SolvableId(id);
                // Get the transaction type
                let id_type = ffi::transaction_type(
                    self.0 .0.as_mut(),
                    id.into(),
                    ffi::SOLVER_TRANSACTION_SHOW_ALL as std::os::raw::c_int,
                );
                // Get the solvable from the pool
                (id.resolve(&pool), InstallOperation::from(id_type as u32))
            };
            solvable_operations.push(OperationOnSolvable {
                solvable,
                operation,
            });
        }
        solvable_operations
    }
}
