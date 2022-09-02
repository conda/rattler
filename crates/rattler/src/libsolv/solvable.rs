use std::ffi::CStr;
use std::ptr::NonNull;

use crate::libsolv::ffi;
use crate::libsolv::pool::{FindInterned, PoolRef, StringId};
use crate::libsolv::repo::RepoRef;

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
    /// Resolves to the interned type returns a Solvable
    pub fn resolve(&self, pool: &PoolRef) -> Solvable {
        // Safe because the new-type wraps the ffi::id and cant be created otherwise
        unsafe {
            // Re-implement pool_id2solvable, as it's a static inline function, we can't use it :(
            let solvables = pool.as_ref().solvables;
            // Apparently the solvable is offset by the id from the first solvable
            let solvable = solvables.offset(self.0 as isize);
            Solvable(NonNull::new(solvable).expect("solvable cannot be null"))
        }
    }
}

#[derive(Debug)]
pub struct SolvableInfo {
    pub name: String,
    pub version: String,
    pub build_string: Option<String>,
    pub build_number: Option<usize>,
}

impl Solvable {
    /// Returns a pointer to the wrapped `ffi::Pool`
    pub(super) fn as_ptr(&self) -> NonNull<ffi::Solvable> {
        self.0
    }

    /// Returns the pool the which this solvable belongs.
    pub fn pool(&self) -> &PoolRef {
        self.repo().pool()
    }

    /// Returns a reference to the Repo that created this instance.
    pub fn repo(&self) -> &RepoRef {
        RepoRef::from_ptr(
            NonNull::new(unsafe { self.0.as_ref() }.repo)
                .expect("the `repo` field of an ffi::Solvable is null"),
        )
    }

    /// Looks up a string value associated with this instance with the given `key`.
    fn lookup_str(&self, key: StringId) -> Option<&str> {
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

    /// Returns a solvable info from a solvable
    pub fn solvable_info(&self) -> SolvableInfo {
        let pool = self.repo().pool();
        let solvable = self.0.as_ptr();

        let id = StringId(unsafe { (*solvable).name });
        let version = StringId(unsafe { (*solvable).evr });

        let solvable_build_flavor = "solvable:buildflavor"
            .find_interned_id(pool)
            .expect("\"solvable:buildflavor\" was not found in the string pool");
        let build_string = self
            .lookup_str(solvable_build_flavor)
            .map(ToOwned::to_owned);

        let solvable_build_version = "solvable:buildversion"
            .find_interned_id(pool)
            .expect("\"solvable:buildversion\" was not found in the string pool");
        let build_number = self.lookup_str(solvable_build_version).map(|num_str| {
            num_str.parse().unwrap_or_else(|e| {
                panic!("could not convert build_number '{num_str}' to number: {e}")
            })
        });

        SolvableInfo {
            name: id
                .resolve(pool)
                .expect("string not found in pool")
                .to_string(),
            version: version
                .resolve(pool)
                .expect("string not found in pool")
                .to_string(),
            build_string,
            build_number,
        }
    }
}
