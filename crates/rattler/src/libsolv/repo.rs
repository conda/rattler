use std::ffi::CString;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

use rattler::RepoData;

use crate::libsolv::pool::PoolRef;
use crate::libsolv::{c_string, ffi, solvable::SolvableId};

use super::pool::{FindInterned, Intern};

// const SOLVABLE_SUMMARY: &str = "solvable:summary";
// const SOLVABLE_DESCRIPTION: &str = "solvable:description";
// const SOLVABLE_DISTRIBUTION: &str = "solvable:distribution";
// const SOLVABLE_AUTHORS: &str = "solvable:authors";
// const SOLVABLE_PACKAGER: &str = "solvable:packager";
// const SOLVABLE_GROUP: &str = "solvable:group";
// const SOLVABLE_URL: &str = "solvable:url";
// const SOLVABLE_KEYWORDS: &str = "solvable:keywords";
const SOLVABLE_LICENSE: &str = "solvable:license";
// const SOLVABLE_BUILDTIME: &str = "solvable:buildtime";
// const SOLVABLE_BUILDHOST: &str = "solvable:buildhost";
// const SOLVABLE_EULA: &str = "solvable:eula";
// const SOLVABLE_CPEID: &str = "solvable:cpeid";
// const SOLVABLE_MESSAGEINS: &str = "solvable:messageins";
// const SOLVABLE_MESSAGEDEL: &str = "solvable:messagedel";
// const SOLVABLE_INSTALLSIZE: &str = "solvable:installsize";
// const SOLVABLE_DISKUSAGE: &str = "solvable:diskusage";
// const SOLVABLE_FILELIST: &str = "solvable:filelist";
// const SOLVABLE_INSTALLTIME: &str = "solvable:installtime";
// const SOLVABLE_MEDIADIR: &str = "solvable:mediadir";
// const SOLVABLE_MEDIAFILE: &str = "solvable:mediafile";
// const SOLVABLE_MEDIANR: &str = "solvable:medianr";
// const SOLVABLE_MEDIABASE: &str = "solvable:mediabase"; /* <location xml:base=... > */
// const SOLVABLE_DOWNLOADSIZE: &str = "solvable:downloadsize";
// const SOLVABLE_SOURCEARCH: &str = "solvable:sourcearch";
// const SOLVABLE_SOURCENAME: &str = "solvable:sourcename";
// const SOLVABLE_SOURCEEVR: &str = "solvable:sourceevr";
// const SOLVABLE_ISVISIBLE: &str = "solvable:isvisible";
// const SOLVABLE_TRIGGERS: &str = "solvable:triggers";
const SOLVABLE_CHECKSUM: &str = "solvable:checksum";
const SOLVABLE_PKGID: &str = "solvable:pkgid"; /* pkgid: md5sum over header + payload */
// const SOLVABLE_HDRID: &str = "solvable:hdrid"; /* hdrid: sha1sum over header only */
// const SOLVABLE_LEADSIGID: &str = "solvable:leadsigid"; /* leadsigid: md5sum over lead + sigheader */
const SOLVABLE_BUILDFLAVOR: &str = "solvable:buildflavor";
const SOLVABLE_BUILDVERSION: &str = "solvable:buildversion";

const REPOKEY_TYPE_MD5: &str = "repokey:type:md5";
// const REPOKEY_TYPE_SHA1: &str = "repokey:type:sha1";
// const REPOKEY_TYPE_SHA224: &str = "repokey:type:sha224";
const REPOKEY_TYPE_SHA256: &str = "repokey:type:sha256";
// const REPOKEY_TYPE_SHA384: &str = "repokey:type:sha384";
// const REPOKEY_TYPE_SHA512: &str = "repokey:type:sha512";

const SOLVABLE_CONSTRAINS: &str = "solvable:constrains"; /* conda */
const SOLVABLE_TRACK_FEATURES: &str = "solvable:track_features"; /* conda */
// const SOLVABLE_ISDEFAULT: &str = "solvable:isdefault";
// const SOLVABLE_LANGONLY: &str = "solvable:langonly";

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

    /// Returns the pool that created this instance
    pub fn pool_mut(&mut self) -> &mut PoolRef {
        // Safe because a `PoolRef` is a wrapper around `ffi::Pool`
        unsafe { &mut *(self.as_ref().pool as *mut PoolRef) }
    }

    /// Adds [`RepoData`] to this instance
    pub fn add_repodata(&mut self, repo_data: &RepoData) -> anyhow::Result<()> {
        let data = unsafe { ffi::repo_add_repodata(self.as_ptr().as_ptr(), 0) };

        // Get all the IDs
        let solvable_buildflavor_id = SOLVABLE_BUILDFLAVOR.find_interned_id(self.pool()).unwrap();
        let solvable_buildversion_id = SOLVABLE_BUILDVERSION.find_interned_id(self.pool()).unwrap();
        let solvable_constraints = SOLVABLE_CONSTRAINS.find_interned_id(self.pool()).unwrap();
        let solvable_license_id = SOLVABLE_LICENSE.find_interned_id(self.pool()).unwrap();
        let solvable_pkg_id = SOLVABLE_PKGID.find_interned_id(self.pool()).unwrap();
        let solvable_checksum = SOLVABLE_CHECKSUM.find_interned_id(self.pool()).unwrap();
        let solvable_track_features = SOLVABLE_TRACK_FEATURES
            .find_interned_id(self.pool())
            .unwrap();
        let repo_type_md5 = REPOKEY_TYPE_MD5.find_interned_id(self.pool()).unwrap();
        let repo_type_sha256 = REPOKEY_TYPE_SHA256.find_interned_id(self.pool()).unwrap();

        // Iterate over all packages
        for (_filename, record) in repo_data.packages.iter() {
            // Create a solvable for the package.
            let solvable_id = SolvableId(unsafe { ffi::repo_add_solvable(self.as_ptr().as_ptr()) });
            let solvable = solvable_id.resolve(self.pool());

            let solvable = unsafe { solvable.as_ptr().as_mut() };

            // Name and version
            solvable.name = record.name.intern(self.pool_mut()).into();
            solvable.evr = record.version.to_string().intern(self.pool_mut()).into();
            solvable.provides = unsafe {
                ffi::repo_addid_dep(
                    self.as_ptr().as_ptr(),
                    solvable.provides,
                    ffi::pool_rel2id(
                        self.pool().as_ptr().as_ptr(),
                        solvable.name,
                        solvable.evr,
                        ffi::REL_EQ as i32,
                        1,
                    ),
                    0,
                )
            };

            // Dependencies
            // TODO: Add requires
            for match_spec in record.depends.iter() {
                // Create a reldep id from a matchspec
                let match_spec_id = unsafe {
                    ffi::pool_conda_matchspec(
                        self.pool().as_ptr().as_ptr(),
                        CString::new(match_spec.as_str())?.as_ptr(),
                    )
                };

                // Add it to the list of requirements of this solvable
                solvable.requires = unsafe {
                    ffi::repo_addid_dep(self.as_ptr().as_ptr(), solvable.requires, match_spec_id, 0)
                };
            }

            // Constraints
            // TODO: Add requires
            for match_spec in record.constrains.iter() {
                // Create a reldep id from a matchspec
                let match_spec_id = unsafe {
                    ffi::pool_conda_matchspec(
                        self.pool().as_ptr().as_ptr(),
                        CString::new(match_spec.as_str())?.as_ptr(),
                    )
                };

                // Add it to the list of constraints of this solvable
                unsafe {
                    ffi::repodata_add_idarray(
                        data,
                        solvable_id.into(),
                        solvable_constraints.into(),
                        match_spec_id,
                    );
                };
            }

            // Track features
            for track_features in record.track_features.iter() {
                let track_feature = track_features.trim();
                if !track_feature.is_empty() {
                    unsafe {
                        ffi::repodata_add_idarray(
                            data,
                            solvable_id.into(),
                            solvable_track_features.into(),
                            track_features.trim().intern(self.pool_mut()).into(),
                        );
                    }
                }
            }

            // Build string
            unsafe {
                ffi::repodata_add_poolstr_array(
                    data,
                    solvable_id.into(),
                    solvable_buildflavor_id.into(),
                    CString::new(record.build.as_str())?.as_ptr(),
                )
            };

            // Build number
            unsafe {
                ffi::repodata_set_str(
                    data,
                    solvable_id.into(),
                    solvable_buildversion_id.into(),
                    CString::new(record.build_number.to_string())?.as_ptr(),
                )
            }

            // License
            if let Some(license) = record.license.as_ref() {
                unsafe {
                    ffi::repodata_add_poolstr_array(
                        data,
                        solvable_id.into(),
                        solvable_license_id.into(),
                        CString::new(license.as_str())?.as_ptr(),
                    )
                }
            }

            // MD5 hash
            if let Some(md5) = record.md5.as_ref() {
                unsafe {
                    ffi::repodata_set_checksum(
                        data,
                        solvable_id.into(),
                        solvable_pkg_id.into(),
                        repo_type_md5.into(),
                        CString::new(md5.as_str())?.as_ptr(),
                    )
                }
            }

            // Sha256 hash
            if let Some(sha256) = record.sha256.as_ref() {
                unsafe {
                    ffi::repodata_set_checksum(
                        data,
                        solvable_id.into(),
                        solvable_checksum.into(),
                        repo_type_sha256.into(),
                        CString::new(sha256.as_str())?.as_ptr(),
                    )
                }
            }
        }

        // TODO: What does this do?
        unsafe { ffi::repo_internalize(self.as_ptr().as_ptr()) };

        Ok(())
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
