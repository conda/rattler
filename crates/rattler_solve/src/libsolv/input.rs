//! Contains business logic that loads information into libsolv in order to solve a conda
//! environment

use crate::libsolv::libc_byte_slice::LibcByteSlice;
use crate::libsolv::wrapper::keys::*;
use crate::libsolv::wrapper::pool::Pool;
use crate::libsolv::wrapper::repo::Repo;
use crate::libsolv::wrapper::repodata::Repodata;
use crate::libsolv::wrapper::solvable::SolvableId;
use rattler_conda_types::package::ArchiveType;
use rattler_conda_types::{GenericVirtualPackage, RepoDataRecord};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::ffi::{CString, NulError};

pub fn add_solv_file(pool: &Pool, repo: &Repo, solv_bytes: &LibcByteSlice) {
    #[cfg(not(target_family = "unix"))]
    unreachable!("this platform does not support in-memory .solv files");

    #[cfg(target_family = "unix")]
    {
        // Add solv file from memory if available
        let mode = CString::new("r").unwrap();
        let file = unsafe { libc::fmemopen(solv_bytes.as_ptr(), solv_bytes.len(), mode.as_ptr()) };
        repo.add_solv(pool, file);
        unsafe { libc::fclose(file) };
    }
}

/// Adds [`RepoDataRecord`] to `repo`
///
/// Panics if the repo does not belong to the pool
pub fn add_repodata_records(
    pool: &Pool,
    repo: &Repo,
    repo_datas: &[RepoDataRecord],
) -> Result<Vec<SolvableId>, NulError> {
    // Sanity check
    repo.ensure_belongs_to_pool(pool);

    // Get all the IDs (these strings are internal to libsolv and always present, so we can
    // unwrap them at will)
    let solvable_buildflavor_id = pool.find_interned_str(SOLVABLE_BUILDFLAVOR).unwrap();
    let solvable_buildtime_id = pool.find_interned_str(SOLVABLE_BUILDTIME).unwrap();
    let solvable_buildversion_id = pool.find_interned_str(SOLVABLE_BUILDVERSION).unwrap();
    let solvable_constraints = pool.find_interned_str(SOLVABLE_CONSTRAINS).unwrap();
    let solvable_download_size_id = pool.find_interned_str(SOLVABLE_DOWNLOADSIZE).unwrap();
    let solvable_license_id = pool.find_interned_str(SOLVABLE_LICENSE).unwrap();
    let solvable_pkg_id = pool.find_interned_str(SOLVABLE_PKGID).unwrap();
    let solvable_checksum = pool.find_interned_str(SOLVABLE_CHECKSUM).unwrap();
    let solvable_track_features = pool.find_interned_str(SOLVABLE_TRACK_FEATURES).unwrap();
    let repo_type_md5 = pool.find_interned_str(REPOKEY_TYPE_MD5).unwrap();
    let repo_type_sha256 = pool.find_interned_str(REPOKEY_TYPE_SHA256).unwrap();

    // Custom id
    let solvable_index_id = pool.intern_str("solvable:repodata_record_index");

    // Keeps a mapping from packages added to the repo to the type and solvable
    let mut package_to_type: HashMap<&str, (ArchiveType, SolvableId)> = HashMap::new();

    // Through `data` we can manipulate solvables (see the `Repodata` docs for details)
    let data = repo.add_repodata();

    let mut solvable_ids = Vec::new();
    for (repo_data_index, repo_data) in repo_datas.iter().enumerate() {
        // Create a solvable for the package
        let solvable_id =
            match add_or_reuse_solvable(pool, repo, &data, &mut package_to_type, repo_data) {
                Some(id) => id,
                None => continue,
            };

        // Store the current index so we can retrieve the original repo data record
        // from the final transaction
        data.set_num(solvable_id, solvable_index_id, repo_data_index as u64);

        // Safe because there are no other active references to any solvable (so no aliasing)
        let solvable = unsafe { solvable_id.resolve_raw(pool).as_mut() };
        let record = &repo_data.package_record;

        // Name and version
        solvable.name = pool.intern_str(record.name.as_str()).into();
        solvable.evr = pool.intern_str(record.version.to_string()).into();
        let rel_eq = pool.rel_eq(solvable.name, solvable.evr);
        repo.add_provides(solvable, rel_eq);

        // Location (filename (fn) and subdir)
        data.set_location(
            solvable_id,
            &CString::new(record.subdir.as_bytes())?,
            &CString::new(repo_data.file_name.as_bytes())?,
        );

        // Dependencies
        for match_spec in record.depends.iter() {
            // Create a reldep id from a matchspec
            let match_spec_id = pool.conda_matchspec(&CString::new(match_spec.as_str())?);

            // Add it to the list of requirements of this solvable
            repo.add_requires(solvable, match_spec_id);
        }

        // Constraints
        for match_spec in record.constrains.iter() {
            // Create a reldep id from a matchspec
            let match_spec_id = pool.conda_matchspec(&CString::new(match_spec.as_str())?);

            // Add it to the list of constraints of this solvable
            data.add_idarray(solvable_id, solvable_constraints, match_spec_id);
        }

        // Track features
        for track_features in record.track_features.iter() {
            let track_feature = track_features.trim();
            if !track_feature.is_empty() {
                data.add_idarray(
                    solvable_id,
                    solvable_track_features,
                    pool.intern_str(track_features.trim()).into(),
                );
            }
        }

        // Timestamp
        if let Some(timestamp) = record.timestamp {
            // Fixup the timestamp
            let timestamp = if timestamp > 253402300799 {
                timestamp / 253402300799
            } else {
                timestamp
            };
            data.set_num(solvable_id, solvable_buildtime_id, timestamp as u64);
        }

        // Size
        if let Some(size) = record.size {
            data.set_num(solvable_id, solvable_download_size_id, size as u64);
        }

        // Build string
        data.add_poolstr_array(
            solvable_id,
            solvable_buildflavor_id,
            &CString::new(record.build.as_str())?,
        );

        // Build number
        data.set_str(
            solvable_id,
            solvable_buildversion_id,
            &CString::new(record.build_number.to_string())?,
        );

        // License
        if let Some(license) = record.license.as_ref() {
            data.add_poolstr_array(
                solvable_id,
                solvable_license_id,
                &CString::new(license.as_str())?,
            );
        }

        // MD5 hash
        if let Some(md5) = record.md5.as_ref() {
            data.set_checksum(
                solvable_id,
                solvable_pkg_id,
                repo_type_md5,
                &CString::new(md5.as_str())?,
            );
        }

        // Sha256 hash
        if let Some(sha256) = record.sha256.as_ref() {
            data.set_checksum(
                solvable_id,
                solvable_checksum,
                repo_type_sha256,
                &CString::new(sha256.as_str())?,
            );
        }

        solvable_ids.push(solvable_id)
    }

    repo.internalize();

    Ok(solvable_ids)
}

/// When adding packages, we want to make sure that `.conda` packages have preference over `.tar.bz`
/// packages. For that reason, when adding a solvable we check first if a `.conda` version of the
/// package has already been added, in which case we forgo adding its `.tar.bz` version (and return
/// `None`). If no `.conda` version has been added, we create a new solvable (replacing any existing
/// solvable for the `.tar.bz` version of the package).
fn add_or_reuse_solvable<'a>(
    pool: &Pool,
    repo: &Repo,
    data: &Repodata,
    package_to_type: &mut HashMap<&'a str, (ArchiveType, SolvableId)>,
    repo_data: &'a RepoDataRecord,
) -> Option<SolvableId> {
    // Sometimes we can reuse an existing solvable
    if let Some((filename, archive_type)) = ArchiveType::split_str(&repo_data.file_name) {
        if let Some(&(other_package_type, old_solvable_id)) = package_to_type.get(filename) {
            match archive_type.cmp(&other_package_type) {
                Ordering::Less => {
                    // A previous package that we already stored is actually a package of a better
                    // "type" so we'll just use that instead (.conda > .tar.bz)
                    return None;
                }
                Ordering::Greater => {
                    // A previous package has a worse package "type", we'll reuse the handle but
                    // overwrite its attributes

                    // Update the package to the new type mapping
                    package_to_type.insert(filename, (archive_type, old_solvable_id));

                    // Reset and reuse the old solvable
                    reset_solvable(pool, repo, data, old_solvable_id);
                    return Some(old_solvable_id);
                }
                Ordering::Equal => {
                    unreachable!("found a duplicate package")
                }
            }
        } else {
            let solvable_id = repo.add_solvable();
            package_to_type.insert(filename, (archive_type, solvable_id));
            return Some(solvable_id);
        }
    } else {
        tracing::warn!("unknown package extension: {}", &repo_data.file_name);
    }

    Some(repo.add_solvable())
}

pub fn add_virtual_packages(
    pool: &Pool,
    repo: &Repo,
    packages: &[GenericVirtualPackage],
) -> Result<(), NulError> {
    let data = repo.add_repodata();

    let solvable_buildflavor_id = pool.find_interned_str(SOLVABLE_BUILDFLAVOR).unwrap();

    for package in packages {
        // Create a solvable for the package
        let solvable_id = repo.add_solvable();

        // Safe because there are no other references to this solvable_id (we just created it)
        let solvable = unsafe { solvable_id.resolve_raw(pool).as_mut() };

        // Name and version
        solvable.name = pool.intern_str(package.name.as_str()).into();
        solvable.evr = pool.intern_str(package.version.to_string()).into();
        let rel_eq = pool.rel_eq(solvable.name, solvable.evr);
        repo.add_provides(solvable, rel_eq);

        // Build string
        data.add_poolstr_array(
            solvable_id,
            solvable_buildflavor_id,
            &CString::new(package.build_string.as_bytes())?,
        );
    }

    Ok(())
}

fn reset_solvable(pool: &Pool, repo: &Repo, data: &Repodata, solvable_id: SolvableId) {
    let blank_solvable = repo.add_solvable();

    // Replace the existing solvable with the blank one
    pool.swap_solvables(blank_solvable, solvable_id);
    data.swap_attrs(blank_solvable, solvable_id);

    // It is safe to free the blank solvable, because there are no other references to it
    // than in this function
    unsafe { repo.free_solvable(blank_solvable) };
}

/// Caches the repodata as an in-memory `.solv` file
///
/// Note: this function relies on primitives that are only available on unix-like operating systems,
/// and will panic if called from another platform (e.g. Windows)
#[cfg(not(target_family = "unix"))]
pub fn cache_repodata(url: String, data: &[RepoDataRecord]) -> LibcByteSlice {
    unimplemented!("this function is only available on unix-like operating systems")
}

/// Caches the repodata as an in-memory `.solv` file
///
/// Note: this function relies on primitives that are only available on unix-like operating systems,
/// and will panic if called from another platform (e.g. Windows)
#[cfg(target_family = "unix")]
pub fn cache_repodata(url: String, data: &[RepoDataRecord]) -> LibcByteSlice {
    // Add repodata to a new pool + repo
    let pool = Pool::default();
    let repo = Repo::new(&pool, url);
    add_repodata_records(&pool, &repo, data).unwrap();

    // Export repo to .solv in memory
    let mut stream_ptr = std::ptr::null_mut();
    let mut stream_size = 0;
    let file = unsafe { libc::open_memstream(&mut stream_ptr, &mut stream_size) };
    if file.is_null() {
        panic!("unable to open memstream");
    }

    repo.write(&pool, file);
    unsafe { libc::fclose(file) };

    let stream_ptr = std::ptr::NonNull::new(stream_ptr).expect("stream_ptr was null");

    // Safe because we know `stream_ptr` points to an array of bytes of length `stream_size`
    unsafe { LibcByteSlice::from_raw_parts(stream_ptr.cast(), stream_size) }
}
