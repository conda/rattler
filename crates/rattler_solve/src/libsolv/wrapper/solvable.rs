use super::ffi;
use super::pool::{Pool, StringId};

/// Represents a solvable in a [`Repo`] or [`Pool`]
#[derive(Copy, Clone)]
pub struct SolvableId(pub(super) ffi::Id);

impl From<SolvableId> for ffi::Id {
    fn from(s: SolvableId) -> Self {
        s.0
    }
}

impl SolvableId {
    /// Resolves to the interned type returns a Solvable
    // TODO: this is actually unsafe, because it allows creating multiple mutable references to the
    // same solvable!
    pub fn resolve<'id, 'pool>(&'id self, pool: &'pool Pool) -> &'pool mut ffi::Solvable {
        // Safe because the new-type wraps the ffi::id and cant be created otherwise
        unsafe {
            // Re-implement pool_id2solvable, as it's a static inline function, we can't use it :(
            let solvables = pool.as_ref().solvables;
            // Apparently the solvable is offset by the id from the first solvable
            let solvable = solvables.offset(self.0 as isize);
            &mut *solvable
        }
    }
}

/// Gets a number associated to this solvable
pub fn lookup_num(solvable: &mut ffi::Solvable, key: StringId) -> Option<u64> {
    let value = unsafe { ffi::solvable_lookup_num(solvable as *mut _, key.0, u64::MAX) };
    if value == u64::MAX {
        None
    } else {
        Some(value)
    }
}
