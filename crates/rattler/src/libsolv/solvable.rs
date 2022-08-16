use crate::libsolv::ffi;
use crate::libsolv::pool::{Intern, Pool, StringId};
use crate::libsolv::repo::{Repo, RepoOwnedPtr};
use std::ffi::{CStr, CString};
use std::mem::ManuallyDrop;
use std::ops::DerefMut;
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
    build_string: Option<String>,
    build_number: Option<String>,
}

impl Solvable {
    /// Access the inner repo
    pub(super) fn repo(&self) -> ManuallyDrop<RepoOwnedPtr> {
        let repo = unsafe { *self.0.as_ptr() }.repo;
        ManuallyDrop::new(unsafe { std::mem::transmute(repo) })
    }

    /// Returns a solvable info from a solvable
    pub fn solvable_info(&self) -> SolvableInfo {
        let mut pool = self.repo().pool();
        let (name, version, build_string, build_number) = unsafe {
            let solvable = self.0.as_ptr();
            let id = StringId((*solvable).name);
            let version = StringId((*solvable).evr);
            let solvable_build_version = "solvable:buildversion".intern(pool.deref_mut());
            let build_str = ffi::solvable_lookup_str(solvable, solvable_build_version.into());
            let build_string = if !build_str.is_null() {
                Some(
                    CStr::from_ptr(build_str)
                        .to_str()
                        .expect("could not decode string")
                        .to_string(),
                )
            } else {
                None
            };
            let solvable_build_flavor = "solvable:buildflavor".intern(pool.deref_mut());
            let build_number = ffi::solvable_lookup_str(solvable, solvable_build_flavor.into());
            let build_number = if !build_number.is_null() {
                Some(
                    CStr::from_ptr(build_number)
                        .to_str()
                        .expect("could not decode string")
                        .to_string(),
                )
            } else {
                None
            };
            (
                id.resolve(&pool),
                version.resolve(&pool),
                build_string,
                build_number,
            )
        };

        SolvableInfo {
            name: name.to_string(),
            version: name.to_string(),
            build_string,
            build_number,
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
