#![cfg_attr(not(target_family = "unix"), allow(dead_code))]

use std::ptr::NonNull;

/// Represents an owned byte slice that was allocated using [`libc::malloc`] and is deallocated upon
/// drop using [`libc::free`].
pub struct LibcByteSlice {
    ptr: NonNull<libc::c_void>,
    len: usize,
}

// We can safely implement `Send` because LibcByteSlice is immutable
unsafe impl Send for LibcByteSlice {}

// We can safely implement `Send` because LibcByteSlice is immutable
unsafe impl Sync for LibcByteSlice {}

impl LibcByteSlice {
    /// Constructs a `LibcByteSlice` from its raw parts
    ///
    /// # Safety
    ///
    /// `ptr` should have been allocated using [`libc::malloc`] and `len` should be the size
    /// in bytes of the allocated chunk of memory
    pub unsafe fn from_raw_parts(ptr: NonNull<libc::c_void>, len: usize) -> LibcByteSlice {
        LibcByteSlice { ptr, len }
    }

    pub(super) fn as_ptr(&self) -> *mut libc::c_void {
        self.ptr.as_ptr()
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }
}

impl Drop for LibcByteSlice {
    fn drop(&mut self) {
        unsafe { libc::free(self.ptr.as_ptr().cast()) }
    }
}
