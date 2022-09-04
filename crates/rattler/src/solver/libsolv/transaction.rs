use super::ffi;
use super::pool::PoolRef;
use super::solvable::{Solvable, SolvableId};
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

/// Enumeration of all install-like operations
#[derive(Debug, Clone, Copy)]
pub enum InstallOperation {
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
            | ffi::SOLVER_TRANSACTION_INSTALL
            | ffi::SOLVER_TRANSACTION_CHANGED => InstallOperation::Install,
            ffi::SOLVER_TRANSACTION_REINSTALL => InstallOperation::Reinstall,
            ffi::SOLVER_TRANSACTION_ERASE => InstallOperation::Remove,
            ffi::SOLVER_TRANSACTION_IGNORE => InstallOperation::Ignore,
            _ => InstallOperation::Unknown,
        }
    }
}

/// Describes what operation should happe
pub struct OperationOnSolvable {
    pub solvable: Solvable,
    pub operation: InstallOperation,
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

    /// Returns the pool that owns this instance.
    pub fn pool(&self) -> &PoolRef {
        // Safe because a `PoolRef` is a wrapper around `ffi::Pool`
        unsafe { &*(self.as_ref().pool as *const PoolRef) }
    }

    /// Returns the solvable operations
    pub fn get_solvable_operations(&mut self) -> Vec<OperationOnSolvable> {
        let mut solvable_operations = Vec::default();
        // Get inner transaction type
        let inner = self.as_ref();
        // Number of transaction details
        let count = inner.steps.count as usize;
        for index in 0..count {
            let (solvable, operation) = unsafe {
                // Get the id for the current solvable
                // Safe because we don't go past the count
                let id = *inner.steps.elements.add(index);
                let id = SolvableId(id);
                // Get the transaction type
                let id_type = ffi::transaction_type(
                    self.as_ptr().as_ptr(),
                    id.into(),
                    ffi::SOLVER_TRANSACTION_SHOW_ALL as std::os::raw::c_int,
                );
                // Get the solvable from the pool
                (
                    id.resolve(self.pool()),
                    InstallOperation::from(id_type as u32),
                )
            };
            solvable_operations.push(OperationOnSolvable {
                solvable,
                operation,
            });
        }
        solvable_operations
    }
}

impl Transaction<'_> {
    /// Constructs a new instance
    pub(super) fn new(ptr: NonNull<ffi::Transaction>) -> Self {
        Transaction(TransactionOwnedPtr(ptr), PhantomData::default())
    }
}
