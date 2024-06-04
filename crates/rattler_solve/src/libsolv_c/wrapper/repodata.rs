use super::{ffi, pool::StringId, repo::Repo, solvable::SolvableId};
use std::{ffi::CStr, marker::PhantomData, ptr::NonNull};

/// Wrapper for libsolv repodata, which provides functions to manipulate solvables
///
/// The wrapper functions as a borrowed pointer, guaranteed to be non-null and living at least as
/// long as the `Repo` it originates from
pub struct Repodata<'repo>(NonNull<ffi::Repodata>, PhantomData<&'repo Repo<'repo>>);

impl Repodata<'_> {
    /// Constructs a new repodata from the provided libsolv pointer. The function will ensure that
    /// the pointer is non-null, but the caller must ensure the pointer is actually valid
    pub(super) unsafe fn from_ptr<'a>(
        _repo: &'a Repo<'a>,
        ptr: *mut ffi::Repodata,
    ) -> Repodata<'a> {
        Repodata(
            NonNull::new(ptr).expect("repodata ptr was null"),
            PhantomData,
        )
    }

    /// Returns a raw pointer to the wrapped [`ffi::Repodata`], to be used for calling ffi functions
    /// that require access to the repodata (and for nothing else)
    pub(super) fn raw_ptr(&self) -> *mut ffi::Repodata {
        self.0.as_ptr()
    }

    /// Calls [`ffi::repodata_set_checksum`]
    pub fn set_checksum(
        &self,
        solvable_id: SolvableId,
        key: StringId,
        checksum_type: StringId,
        value: &CStr,
    ) {
        unsafe {
            ffi::repodata_set_checksum(
                self.raw_ptr(),
                solvable_id.into(),
                key.into(),
                checksum_type.into(),
                value.as_ptr(),
            );
        }
    }

    /// Calls [`ffi::repodata_set_location`]
    pub fn set_location(&self, solvable_id: SolvableId, dir: &CStr, file: &CStr) {
        unsafe {
            ffi::repodata_set_location(
                self.raw_ptr(),
                solvable_id.into(),
                0,
                dir.as_ptr(),
                file.as_ptr(),
            );
        }
    }

    /// Calls [`ffi::repodata_set_num`]
    pub fn set_num(&self, solvable_id: SolvableId, key: StringId, value: u64) {
        unsafe {
            ffi::repodata_set_num(self.raw_ptr(), solvable_id.into(), key.into(), value);
        }
    }

    /// Calls [`ffi::repodata_set_str`]
    pub fn set_str(&self, solvable_id: SolvableId, key: StringId, value: &CStr) {
        unsafe {
            ffi::repodata_set_str(
                self.raw_ptr(),
                solvable_id.into(),
                key.into(),
                value.as_ptr(),
            );
        }
    }

    /// Calls [`ffi::repodata_add_idarray`]
    pub fn add_idarray(&self, solvable_id: SolvableId, array_key: StringId, id: ffi::Id) {
        unsafe {
            ffi::repodata_add_idarray(self.raw_ptr(), solvable_id.into(), array_key.into(), id);
        }
    }

    /// Calls [`ffi::repodata_add_poolstr_array`]
    pub fn add_poolstr_array(&self, solvable_id: SolvableId, key: StringId, value: &CStr) {
        unsafe {
            ffi::repodata_add_poolstr_array(
                self.raw_ptr(),
                solvable_id.into(),
                key.into(),
                value.as_ptr(),
            );
        };
    }

    /// Calls [`ffi::repodata_swap_attrs`]
    pub fn swap_attrs(&self, s1: SolvableId, s2: SolvableId) {
        unsafe {
            ffi::repodata_swap_attrs(self.raw_ptr(), s1.into(), s2.into());
        }
    }
}
