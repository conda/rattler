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
    ///
    /// TODO: should be unsafe!
    pub fn resolve(self, pool: &Pool) -> &mut ffi::Solvable {
        // Note: this is a reimplementation of pool_id2solvable, which is not included in the bindings
        // because it is static inline
        let solvables = pool.as_ref().solvables;

        // Internally, the id is an offset to be applied on top of `pool.solvables`
        unsafe { &mut *solvables.offset(self.0 as isize) }
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
