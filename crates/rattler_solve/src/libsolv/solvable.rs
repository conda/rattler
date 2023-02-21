use std::ffi::CStr;
use std::marker::PhantomData;
use std::ptr::NonNull;
use url::Url;

use super::custom_keys::SOLVABLE_REAL_URL;
use super::ffi;
use super::keys::*;
use super::pool::{FindInterned, PoolRef, StringId};
use super::repo::RepoRef;

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
    pub fn resolve(&self, pool: &PoolRef) -> Solvable {
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
    /// Returns a pointer to the wrapped `ffi::Pool`
    pub(super) fn as_ptr(&self) -> NonNull<ffi::Solvable> {
        self.0
    }

    /// Returns the pool the which this solvable belongs.
    pub fn pool(&self) -> &PoolRef {
        self.repo().pool()
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
    pub fn repo(&self) -> &RepoRef {
        // Safe because a `RepoRef` is a wrapper around `ffi::Repo`
        unsafe { &*(self.as_ptr().as_ref().repo as *const RepoRef) }
    }

    pub fn url(&self) -> Url {
        let s = self.resolve_by_key(SOLVABLE_REAL_URL).unwrap().to_string();
        Url::parse(&s).unwrap()
    }

    /// Returns the location of the solvable which is defined by the subdirectory and the filename of the package.
    pub fn location(&self) -> Option<String> {
        unsafe {
            let loc = ffi::solvable_get_location(self.as_ptr().as_ptr(), std::ptr::null_mut());
            if loc.is_null() {
                None
            } else {
                let str = CStr::from_ptr(loc)
                    .to_str()
                    .expect("invalid utf8 in location")
                    .to_owned();

                Some(str)
            }
        }
    }

    pub fn name(&self) -> String {
        self.resolve_by_id(|s| s.name)
    }

    pub fn version(&self) -> String {
        self.resolve_by_id(|s| s.evr)
    }

    pub fn build_string(&self) -> Option<String> {
        self.resolve_by_key(SOLVABLE_BUILDFLAVOR)
            .map(ToOwned::to_owned)
    }

    pub fn build_number(&self) -> Option<usize> {
        self.resolve_by_key(SOLVABLE_BUILDVERSION).map(|num_str| {
            num_str.parse().unwrap_or_else(|e| {
                panic!("could not convert build_number '{num_str}' to number: {e}")
            })
        })
    }

    pub fn md5(&self) -> Option<String> {
        let checksum_id = SOLVABLE_PKGID.find_interned_id(self.pool()).unwrap();
        let expected_checksum_type = REPOKEY_TYPE_MD5.find_interned_id(self.pool()).unwrap();
        self.lookup_checksum(checksum_id, expected_checksum_type)
    }

    pub fn sha256(&self) -> Option<String> {
        let checksum_id = SOLVABLE_CHECKSUM.find_interned_id(self.pool()).unwrap();
        let expected_checksum_type = REPOKEY_TYPE_SHA256.find_interned_id(self.pool()).unwrap();
        self.lookup_checksum(checksum_id, expected_checksum_type)
    }

    fn lookup_checksum(
        &self,
        key_name: StringId,
        expected_checksum_type: StringId,
    ) -> Option<String> {
        let mut checksum_type: ffi::Id = -1;

        let checksum = unsafe {
            ffi::solvable_lookup_checksum(self.as_ptr().as_ptr(), key_name.0, &mut checksum_type)
        };

        if checksum.is_null() {
            None
        } else {
            if checksum_type != expected_checksum_type.0 {
                panic!("libsolv returned the wrong checksum type! {checksum_type}");
            }

            // Safe to unwrap, because the checksum is valid UTF8
            let checksum = unsafe { CStr::from_ptr(checksum) }
                .to_str()
                .unwrap()
                .to_string();

            Some(checksum)
        }
    }

    fn resolve_by_key(&self, key: &str) -> Option<&str> {
        let id = key.find_interned_id(self.pool());
        match id {
            None => panic!("key `{key}` was not found in the string pool"),
            Some(id) => self.lookup_str(id),
        }
    }

    fn resolve_by_id(&self, get_id: impl Fn(ffi::Solvable) -> ffi::Id) -> String {
        let id = (get_id)(unsafe { *self.0.as_ptr() });
        let string_id = StringId(id);
        string_id
            .resolve(self.pool())
            .expect("string not found in pool")
            .to_string()
    }
}
