use std::ffi::CStr;
use std::marker::PhantomData;
use std::ptr::NonNull;

use super::ffi;
use super::pool::{Pool, StringId};
use super::repo::RepoId;

/// Solvable in libsolv
#[repr(transparent)]
pub struct Solvable<'repo>(NonNull<ffi::Solvable>, PhantomData<&'repo ffi::Repo>);

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
    pub fn resolve(&self, pool: &Pool) -> Solvable {
        // Safe because the new-type wraps the ffi::id and cant be created otherwise
        unsafe {
            // Re-implement pool_id2solvable, as it's a static inline function, we can't use it :(
            let solvables = pool.as_ref().solvables;
            // Apparently the solvable is offset by the id from the first solvable
            let solvable = solvables.offset(self.0 as isize);
            Solvable(
                NonNull::new(solvable).expect("solvable cannot be null"),
                PhantomData::default(),
            )
        }
    }
}

impl<'repo> Solvable<'repo> {
    /// Returns a pointer to the wrapped `ffi::Solvable`
    pub(super) fn as_ptr(&self) -> NonNull<ffi::Solvable> {
        self.0
    }

    /// Looks up a string value associated with this instance with the given `key`.
    pub fn lookup_str(&self, key: StringId) -> Option<&str> {
        let str = unsafe { ffi::solvable_lookup_str(self.0.as_ptr(), key.into()) };
        if str.is_null() {
            None
        } else {
            unsafe {
                Some(
                    CStr::from_ptr(str)
                        .to_str()
                        .expect("could not decode string"),
                )
            }
        }
    }

    /// Get the repo to which this solvable belongs.
    pub fn repo_id(&self) -> RepoId {
        let solvable = unsafe { self.as_ptr().as_ref() };
        RepoId::from_solvable_struct(solvable)
    }

    pub fn set_usize(&self, key: StringId, x: usize) {
        unsafe { ffi::solvable_set_num(self.as_ptr().as_ptr(), key.0, x as u64) };
    }

    pub fn get_usize(&self, key: StringId) -> Option<usize> {
        let value = unsafe { ffi::solvable_lookup_num(self.as_ptr().as_ptr(), key.0, u64::MAX) };
        if value == u64::MAX {
            None
        } else {
            Some(value as usize)
        }
    }
}
