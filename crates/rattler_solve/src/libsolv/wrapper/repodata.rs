use super::ffi;
use crate::libsolv::wrapper::pool::StringId;
use crate::libsolv::wrapper::solvable::SolvableId;
use std::ffi::CStr;
use std::ptr::NonNull;

// TODO: do we need drop?
// TODO: can we tie this to a lifetime?
pub struct Repodata(NonNull<ffi::Repodata>);

impl Repodata {
    /// Constructs a new repodata from the provided libsolv pointer. The function will ensure that
    /// the pointer is non-null, but the caller must ensure the pointer is actually valid
    pub(super) unsafe fn from_ptr(ptr: *mut ffi::Repodata) -> Repodata {
        Repodata(NonNull::new(ptr).expect("repodata ptr was null"))
    }

    /// Returns a raw pointer to the wrapped `ffi::Repodata`, to be used for calling ffi functions
    /// that require access to the repodata (and for nothing else)
    pub(super) fn raw_ptr(&self) -> *mut ffi::Repodata {
        self.0.as_ptr()
    }

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
            )
        }
    }

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

    pub fn set_num(&self, solvable_id: SolvableId, key: StringId, value: u64) {
        unsafe {
            ffi::repodata_set_num(self.raw_ptr(), solvable_id.into(), key.into(), value);
        }
    }

    pub fn set_str(&self, solvable_id: SolvableId, key: StringId, value: &CStr) {
        unsafe {
            ffi::repodata_set_str(
                self.raw_ptr(),
                solvable_id.into(),
                key.into(),
                value.as_ptr(),
            )
        }
    }

    pub fn add_idarray(&self, solvable_id: SolvableId, array_key: StringId, id: ffi::Id) {
        unsafe {
            ffi::repodata_add_idarray(self.raw_ptr(), solvable_id.into(), array_key.into(), id);
        }
    }

    pub fn add_poolstr_array(&self, solvable_id: SolvableId, key: StringId, value: &CStr) {
        unsafe {
            ffi::repodata_add_poolstr_array(
                self.raw_ptr(),
                solvable_id.into(),
                key.into(),
                value.as_ptr(),
            )
        };
    }

    pub fn swap_attrs(&self, s1: SolvableId, s2: SolvableId) {
        unsafe {
            ffi::repodata_swap_attrs(self.raw_ptr(), s1.into(), s2.into());
        }
    }
}
