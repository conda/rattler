use crate::libsolv::{c_string, ffi};
use std::marker::PhantomData;
use std::ptr::NonNull;

/// Representation of a repo containing package data in libsolv
/// This corresponds to a repo_data json
/// Lifetime of this object is coupled to the Pool on creation
pub struct Repo<'pool>(
    pub(super) NonNull<ffi::Repo>,
    pub(super) PhantomData<&'pool ffi::Pool>,
);

/// Destroy c side of things when repo is dropped
impl Drop for Repo<'_> {
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
            let ret = ffi::repo_add_conda(self.0.as_mut(), file, 0);
            if ret != 0 {
                return Err(anyhow::anyhow!(
                    "internal libsolv error while adding repodata to libsolv"
                ));
            }
        }
        Ok(())
    }
}
