use crate::libsolv::ffi;
use std::marker::PhantomData;
use std::ptr::NonNull;

/// Wrapper type so we do not use lifetime in the drop
pub(super) struct TransactionOwnedPtr(NonNull<ffi::Transaction>);

impl Drop for TransactionOwnedPtr {
    fn drop(&mut self) {
        unsafe { ffi::transaction_free(self.0.as_mut()) }
    }
}
impl TransactionOwnedPtr {
    pub fn new(transaction: *mut ffi::Transaction) -> Self {
        Self(NonNull::new(transaction).expect("could not create transaction object"))
    }
}

pub struct Transaction<'solver>(
    pub(super) TransactionOwnedPtr,
    pub(super) PhantomData<&'solver ffi::Solver>,
);
