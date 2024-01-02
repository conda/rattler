use super::{
    ffi,
    pool::{Pool, StringId},
};
use std::ptr::NonNull;

/// Represents a solvable in a [`Repo`] or [`Pool`]
#[derive(Copy, Clone, Debug)]
pub struct SolvableId(pub(super) ffi::Id);

impl From<SolvableId> for ffi::Id {
    fn from(s: SolvableId) -> Self {
        s.0
    }
}

impl SolvableId {
    /// Resolves the id to a pointer to the solvable
    ///
    /// Panics if the solvable is not found in the pool
    pub fn resolve_raw(self, pool: &Pool) -> NonNull<ffi::Solvable> {
        let pool = pool.as_ref();

        // Internally, the id is just an offset to be applied on top of `pool.solvables`
        if self.0 < pool.nsolvables {
            // Safe because we just checked the offset is within bounds
            let solvable_ptr = unsafe { pool.solvables.offset(self.0 as isize) };
            NonNull::new(solvable_ptr).expect("solvable ptr was null")
        } else {
            panic!("invalid solvable id!")
        }
    }
}

/// Gets a number associated to this solvable
pub fn lookup_num(solvable: *mut ffi::Solvable, key: StringId) -> Option<u64> {
    let value = unsafe { ffi::solvable_lookup_num(solvable.cast(), key.0, u64::MAX) };
    if value == u64::MAX {
        None
    } else {
        Some(value)
    }
}
