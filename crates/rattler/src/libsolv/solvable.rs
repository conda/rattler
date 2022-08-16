use crate::libsolv::ffi;
use crate::libsolv::pool::{Pool, StringId};
use crate::libsolv::repo::{Repo, RepoOwnedPtr};
use std::mem::ManuallyDrop;
use std::ptr::NonNull;

/// Solvable in libsolv
#[repr(transparent)]
pub struct Solvable(NonNull<ffi::Solvable>);

/// Represents a solvable in a [`Repo`] or [`Pool`]
#[derive(Copy, Clone)]
pub struct SolvableId(pub(super) ffi::Id);

impl From<SolvableId> for ffi::Id {
    fn from(s: SolvableId) -> Self {
        s.0
    }
}

impl SolvableId {
    /// Resolve to the interned type returns a Solvable
    pub fn resolve(&self, pool: &Pool) -> Solvable {
        // Safe because the new-type wraps the ffi::id and cant be created otherwise
        unsafe {
            // Re-implement pool_id2solvable, as it's a static inline function, we can't use it :(
            let solvable = (*pool.0.as_ptr()).solvables;
            // Apparently the solvable is offset by the id from the first solvable
            let solvable = solvable.offset(self.0 as isize);
            Solvable(NonNull::new(solvable).expect("solvable cannot be null"))
        }
    }
}

#[derive(Debug)]
pub struct SolvableInfo {
    name: String,
    version: String,
}

impl Solvable {
    /// Access the inner repo
    pub(super) fn repo(&self) -> ManuallyDrop<RepoOwnedPtr> {
        let repo = unsafe { *self.0.as_ptr() }.repo;
        ManuallyDrop::new(unsafe { std::mem::transmute(repo) })
    }

    pub fn solvable_info(&self) -> SolvableInfo {
        let pool = self.repo().pool();
        let (name, version) = unsafe {
            let id = StringId((*self.0.as_ptr()).name);
            let version = StringId((*self.0.as_ptr()).evr);

            (id.resolve(&pool), version.resolve(&pool))
        };

        SolvableInfo {
            name: name.to_string(),
            version: name.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::libsolv::pool::Pool;

    #[test]
    pub fn test_solvable_creation() {
        let mut pool = Pool::default();
        let mut repo = pool.create_repo("bla");
        let solvable_id = repo.add_solvable();
        let _solvable = solvable_id.resolve(&pool);
    }
}
