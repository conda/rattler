use super::repodata::Repodata;

use std::{marker::PhantomData, ptr::NonNull};

use super::{c_string, ffi, pool::Pool, solvable::SolvableId};

/// Wrapper for libsolv repo, which contains package information (in our case, we are creating repos
/// from `repodata.json`, installed package metadata and virtual packages)
///
/// The wrapper functions as an owned pointer, guaranteed to be non-null and freed
/// when the Repo is dropped. Next to that, it is also tied to the lifetime of the pool that created
/// it, because libsolv requires a pool to outlive its repos.
pub struct Repo<'pool>(NonNull<ffi::Repo>, PhantomData<&'pool ffi::Repo>);

/// An Id to uniquely identify a Repo
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct RepoId(usize);

impl RepoId {
    pub fn from_ffi_solvable(solvable: &ffi::Solvable) -> RepoId {
        RepoId(solvable.repo as usize)
    }
}

impl<'pool> Drop for Repo<'pool> {
    fn drop(&mut self) {
        // Safe because we know that the repo is never freed manually
        unsafe { ffi::repo_free(self.0.as_mut(), 1) }
    }
}

impl<'pool> Repo<'pool> {
    /// Constructs a repo in the provided pool, associated to the given url
    pub fn new(pool: &Pool, url: impl AsRef<str>) -> Repo {
        let c_url = c_string(url);

        unsafe {
            let repo_ptr = ffi::repo_create(pool.raw_ptr(), c_url.as_ptr());
            let non_null_ptr = NonNull::new(repo_ptr).expect("repo ptr was null");
            Repo(non_null_ptr, PhantomData::default())
        }
    }

    /// Returns the id of the Repo
    pub fn id(&self) -> RepoId {
        RepoId(self.raw_ptr() as usize)
    }

    /// Returns a raw pointer to the wrapped `ffi::Repo`, to be used for calling ffi functions
    /// that require access to the repo (and for nothing else)
    pub fn raw_ptr(&self) -> *mut ffi::Repo {
        self.0.as_ptr()
    }

    /// Adds a new repodata to this repo
    pub fn add_repodata(&self) -> Repodata {
        unsafe {
            let repodata_ptr = ffi::repo_add_repodata(self.raw_ptr(), 0);
            Repodata::from_ptr(self, repodata_ptr)
        }
    }

    /// Adds a new solvable to this repo
    pub fn add_solvable(&self) -> SolvableId {
        SolvableId(unsafe { ffi::repo_add_solvable(self.raw_ptr()) })
    }

    /// Adds a new "requires" relation to the solvable
    pub fn add_requires(&self, solvable: &mut ffi::Solvable, rel_id: ffi::Id) {
        solvable.requires =
            unsafe { ffi::repo_addid_dep(self.raw_ptr(), solvable.requires, rel_id, 0) };
    }

    /// Adds a new "provides" relation to the solvable
    pub fn add_provides(&self, solvable: &mut ffi::Solvable, rel_id: ffi::Id) {
        solvable.provides =
            unsafe { ffi::repo_addid_dep(self.raw_ptr(), solvable.provides, rel_id, 0) };
    }

    /// Wrapper around `repo_internalize`
    pub fn internalize(&self) {
        unsafe { ffi::repo_internalize(self.raw_ptr()) }
    }

    /// Frees the solvable
    ///
    /// The caller must ensure the solvable referenced by this id will not be used in the future
    pub unsafe fn free_solvable(&self, solvable_id: SolvableId) {
        ffi::repo_free_solvable(self.raw_ptr(), solvable_id.into(), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::super::pool::Pool;
    use super::Repo;

    #[test]
    fn test_repo_creation() {
        let pool = Pool::default();
        let mut _repo = Repo::new(&pool, "conda-forge");
    }
}
