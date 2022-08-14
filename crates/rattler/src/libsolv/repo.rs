use crate::libsolv::pool::Pool;
use crate::libsolv::solvable::{Solvable, SolvableId};
use crate::libsolv::{c_string, ffi};
use std::marker::PhantomData;
use std::ptr::NonNull;

/// Representation of a repo containing package data in libsolv
/// This corresponds to a repo_data json
/// Lifetime of this object is coupled to the Pool on creation
pub struct Repo<'pool>(pub(super) RepoOwnedPtr, pub(super) PhantomData<&'pool Pool>);

/// Wrapper type so we do not use lifetime in the drop
pub(super) struct RepoOwnedPtr(NonNull<ffi::Repo>);

impl RepoOwnedPtr {
    pub fn new(repo: *mut ffi::Repo) -> RepoOwnedPtr {
        Self(NonNull::new(repo).expect("Could not create repo object"))
    }
}

/// Destroy c-side of things when repo is dropped
impl Drop for RepoOwnedPtr {
    // Safe because we have coupled Repo lifetime to Pool lifetime
    fn drop(&mut self) {
        unsafe { ffi::repo_free(self.0.as_mut(), 1) }
    }
}

impl Repo<'_> {
    /// Add conda json to the repo
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
            let ret = ffi::repo_add_conda(self.0 .0.as_mut(), file, 0);
            if ret != 0 {
                return Err(anyhow::anyhow!(
                    "internal libsolv error while adding repodata to libsolv"
                ));
            }

            // Libsolv needs this function to be called so we can work with the repo later
            // TODO: maybe wolf knows more about this function
            ffi::repo_internalize(self.0 .0.as_mut());
        }
        Ok(())
    }

    /// Add a solvable to the Repo
    pub fn add_solvable(&mut self) -> SolvableId {
        unsafe { SolvableId(ffi::repo_add_solvable(self.0 .0.as_mut())) }
    }
}

#[cfg(test)]
mod tests {
    use crate::libsolv::pool::Pool;

    #[test]
    fn test_repo_creation() {
        let mut pool = Pool::default();
        let mut repo = pool.create_repo("conda-forge");
    }
}
