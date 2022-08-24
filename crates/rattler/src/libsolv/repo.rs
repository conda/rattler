use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

use crate::libsolv::pool::PoolRef;
use crate::libsolv::solvable::SolvableId;
use crate::libsolv::{c_string, ffi};

/// Representation of a repo containing package data in libsolv. This corresponds to a repo_data
/// json. Lifetime of this object is coupled to the Pool on creation
pub struct Repo<'pool>(RepoOwnedPtr, PhantomData<&'pool ffi::Repo>);

#[repr(transparent)]
pub struct RepoRef(ffi::Repo);

impl<'pool> Deref for Repo<'pool> {
    type Target = RepoRef;

    fn deref(&self) -> &Self::Target {
        unsafe { self.0 .0.cast().as_ref() }
    }
}

impl<'pool> DerefMut for Repo<'pool> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.0 .0.cast().as_mut() }
    }
}

/// Wrapper type so we do not use lifetime in the drop
struct RepoOwnedPtr(NonNull<ffi::Repo>);

/// Destroy c-side of things when repo is dropped
impl Drop for RepoOwnedPtr {
    // Safe because we have coupled Repo lifetime to Pool lifetime
    fn drop(&mut self) {
        unsafe { ffi::repo_free(self.0.as_mut(), 1) }
    }
}

impl RepoRef {
    /// Converts from a pointer to an `ffi::Repo` to a reference of Self.
    pub(super) fn from_ptr<'pool>(repo: NonNull<ffi::Repo>) -> &'pool Self {
        // Safe because a `RepoRef` is a transparent wrapper around `ffi::Repo`
        unsafe { std::mem::transmute(repo.as_ref()) }
    }

    /// Returns a pointer to the wrapped `ffi::Repo`
    fn as_ptr(&self) -> NonNull<ffi::Repo> {
        // Safe because a `RepoRef` is a transparent wrapper around `ffi::Repo`
        unsafe { NonNull::new_unchecked(self as *const Self as *mut Self).cast() }
    }

    /// Returns a reference to the wrapped `ffi::Repo`.
    fn as_ref(&self) -> &ffi::Repo {
        // Safe because a `RepoRef` is a transparent wrapper around `ffi::Repo`
        unsafe { std::mem::transmute(self) }
    }

    /// Returns the pool that created this instance
    pub fn pool(&self) -> &PoolRef {
        // Safe because a `PoolRef` is a wrapper around `ffi::Pool`
        unsafe { &*(self.as_ref().pool as *const PoolRef) }
    }

    /// Reads the content of the file pointed to by `json_path` and adds it to the instance.
    pub fn add_conda_json<T: AsRef<str>>(&mut self, json_path: T) -> anyhow::Result<()> {
        let c_json = c_string(json_path.as_ref());
        let mode = c_string("r");
        unsafe {
            // Cast needed because types do not match in bindgen
            // TODO: see if lib types could be used in bindgen
            // Safe because we check nullptr
            let file = libc::fopen(c_json.as_ptr(), mode.as_ptr()) as *mut ffi::FILE;
            if file.is_null() {
                return Err(anyhow::anyhow!(
                    "fopen returned a nullptr. '{}' does this file exist?",
                    json_path.as_ref()
                ));
            }
            // This line could crash if the json is malformed
            let ret = ffi::repo_add_conda(self.as_ptr().as_mut(), file, 0);
            if ret != 0 {
                return Err(anyhow::anyhow!(
                    "internal libsolv error while adding repodata to libsolv"
                ));
            }

            // Libsolv needs this function to be called so we can work with the repo later
            // TODO: maybe wolf knows more about this function
            ffi::repo_internalize(self.as_ptr().as_mut());
        }
        Ok(())
    }

    /// Adds a `Solvable` to this instance
    pub fn add_solvable(&mut self) -> SolvableId {
        unsafe { SolvableId(ffi::repo_add_solvable(self.as_ptr().as_ptr())) }
    }
}

impl Repo<'_> {
    /// Constructs a new instance
    pub(super) fn new(ptr: NonNull<ffi::Repo>) -> Self {
        Repo(RepoOwnedPtr(ptr), PhantomData::default())
    }
}

#[cfg(test)]
mod tests {
    use crate::libsolv::pool::Pool;

    #[test]
    fn test_repo_creation() {
        let mut pool = Pool::default();
        let mut _repo = pool.create_repo("conda-forge");
    }
}
