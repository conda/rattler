use super::{ffi, queue::QueueRef, solvable::SolvableId, solver::Solver};
use std::{marker::PhantomData, ptr::NonNull};

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
        Transaction(ptr, PhantomData)
    }

    /// Returns a raw pointer to the wrapped `ffi::Transaction`, to be used for calling ffi functions
    /// that require access to the repodata (and for nothing else)
    fn raw_ptr(&self) -> *mut ffi::Transaction {
        self.0.as_ptr()
    }

    /// Returns a reference to the wrapped `ffi::Transaction`.
    pub fn as_ref(&self) -> &ffi::Transaction {
        unsafe { self.0.as_ref() }
    }

    /// Returns the transaction type
    pub fn transaction_type(&self, solvable_id: SolvableId) -> ffi::Id {
        unsafe {
            ffi::transaction_type(
                self.raw_ptr(),
                solvable_id.into(),
                ffi::SOLVER_TRANSACTION_SHOW_ALL as std::os::raw::c_int,
            )
        }
    }

    /// Returns the second solvable associated to the transaction
    ///
    /// Safety: the caller must ensure the transaction has an associated solvable and that the
    /// provided `solvable_id` is valid
    pub unsafe fn obs_pkg(&self, solvable_id: SolvableId) -> SolvableId {
        SolvableId(unsafe { ffi::transaction_obs_pkg(self.raw_ptr(), solvable_id.into()) })
    }

    /// Returns the transaction's queue, containing a solvable id for each transaction
    pub fn get_steps(&self) -> QueueRef<'_> {
        // Safe because the transaction is live and `transaction.steps` is a queue
        unsafe { QueueRef::from_ffi_queue(self, self.as_ref().steps) }
    }
}
