use super::{super::c_string, ffi, pool::Pool, repodata::Repodata, solvable::SolvableId};
use std::{marker::PhantomData, ptr::NonNull};

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
    pub fn new(pool: &Pool, url: impl AsRef<str>, priority: i32) -> Repo<'_> {
        let c_url = c_string(url);

        unsafe {
            let repo_ptr = ffi::repo_create(pool.raw_ptr(), c_url.as_ptr());
            let mut non_null_ptr = NonNull::new(repo_ptr).expect("repo ptr was null");
            non_null_ptr.as_mut().priority = priority;
            Repo(non_null_ptr, PhantomData)
        }
    }

    /// Panics if this repo does not belong to the provided pool
    pub fn ensure_belongs_to_pool(&self, pool: &Pool) {
        let repo_pool_ptr = unsafe { self.0.as_ref().pool };
        assert_eq!(
            repo_pool_ptr,
            pool.raw_ptr(),
            "repo does not belong to the provided pool"
        );
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

    /// Adds a new repodata to this repo (repodata is a libsolv datastructure, see [`Repodata`] for
    /// details)
    pub fn add_repodata(&self) -> Repodata<'_> {
        unsafe {
            let repodata_ptr = ffi::repo_add_repodata(self.raw_ptr(), 0);
            Repodata::from_ptr(self, repodata_ptr)
        }
    }

    /// Adds a `.solv` file to the repo
    pub fn add_solv(&self, pool: &Pool, file: *mut libc::FILE) {
        let result = unsafe { ffi::repo_add_solv(self.raw_ptr(), file.cast(), 0) };
        assert_eq!(result, 0, "add_solv failed: {}", pool.last_error());
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

    /// Serializes the current repo as a `.solv` file
    ///
    /// The provided file should have been opened with write access. Closing the file is the
    /// responsibility of the caller.
    pub fn write(&self, pool: &Pool, file: *mut libc::FILE) {
        let result = unsafe { ffi::repo_write(self.raw_ptr(), file.cast()) };
        assert_eq!(result, 0, "repo_write failed: {}", pool.last_error());
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
    use super::{super::pool::StringId, *};

    #[test]
    fn test_repo_creation() {
        let pool = Pool::default();
        let mut _repo = Repo::new(&pool, "conda-forge", 0);
    }

    #[test]
    fn test_repo_solv_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let solv_path = dir.path().join("repo.solv").to_string_lossy().into_owned();
        let solv_path = c_string(solv_path);

        {
            // Create a pool and a repo
            let pool = Pool::default();
            let repo = Repo::new(&pool, "conda-forge", 0);

            // Add a solvable with a particular name
            let solvable_id = repo.add_solvable();
            let solvable = unsafe { solvable_id.resolve_raw(&pool).as_mut() };
            solvable.name = pool.intern_str("dummy-solvable").into();

            // Open and write the .solv file
            let mode = c_string("wb");
            let file = unsafe { libc::fopen(solv_path.as_ptr(), mode.as_ptr()) };
            repo.write(&pool, file);
            unsafe { libc::fclose(file) };
        }

        // Create a clean pool and repo
        let pool = Pool::default();
        let repo = Repo::new(&pool, "conda-forge", 0);

        // Open and read the .solv file
        let mode = c_string("rb");
        let file = unsafe { libc::fopen(solv_path.as_ptr(), mode.as_ptr()) };
        repo.add_solv(&pool, file);
        unsafe { libc::fclose(file) };

        // Check that everything was properly loaded
        let repo = unsafe { repo.0.as_ref() };
        assert_eq!(repo.nsolvables, 1);

        let ffi_pool = pool.as_ref();

        // Somehow there are already 2 solvables in the pool, so we check at the third position
        let solvable = unsafe { *ffi_pool.solvables.offset(2) };
        let name = StringId(solvable.name).resolve(&pool).unwrap();
        assert_eq!(name, "dummy-solvable");
    }
}
