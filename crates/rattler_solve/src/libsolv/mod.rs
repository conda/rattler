use rattler_conda_types::{GenericVirtualPackage, RepoDataRecord};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::ffi::{CString, NulError};

mod wrapper;

use crate::libsolv::wrapper::keys::*;
use crate::libsolv::wrapper::repo::Repo;
use crate::libsolv::wrapper::repodata::Repodata;
use crate::libsolv::wrapper::solvable::SolvableId;
use crate::{PackageOperation, SolveError, SolverBackend, SolverProblem};
use wrapper::flags::{SolvableFlags, SolverFlag};
use wrapper::pool::{Pool, Verbosity};
use wrapper::queue::Queue;

/// A [`SolverBackend`] implemented using the `libsolv` library
pub struct LibsolvSolver;

impl SolverBackend for LibsolvSolver {
    fn solve(&mut self, problem: SolverProblem) -> Result<Vec<PackageOperation>, SolveError> {
        // Construct a default libsolv pool
        let pool = Pool::default();

        // Setup proper logging for the pool
        pool.set_debug_callback(|msg, flags| {
            tracing::event!(tracing::Level::DEBUG, flags, "{}", msg);
        });
        pool.set_debug_level(Verbosity::Low);

        // Create repos for all channels
        let mut repo_mapping = HashMap::with_capacity(problem.available_packages.len() + 1);
        let mut all_repodata_records = Vec::with_capacity(repo_mapping.len());
        for repodata_records in &problem.available_packages {
            if repodata_records.is_empty() {
                continue;
            }

            let channel_name = &repodata_records[0].channel;
            let repo = Repo::new(&pool, channel_name);
            add_repodata_records(&pool, &repo, repodata_records)
                .map_err(SolveError::ErrorAddingRepodata)?;

            // Keep our own info about repodata_records
            let i = repo_mapping.len();
            repo_mapping.insert(repo.id(), i);
            all_repodata_records.push(repodata_records.as_slice());

            // We dont want to drop the Repo, its stored in the pool anyway, so just forget it.
            std::mem::forget(repo);
        }

        // Installed and virtual packages
        let repo = Repo::new(&pool, "installed");
        let installed_records: Vec<_> = problem
            .installed_packages
            .into_iter()
            .map(|p| p.repodata_record)
            .collect();
        add_repodata_records(&pool, &repo, &installed_records)
            .map_err(SolveError::ErrorAddingInstalledPackages)?;
        add_virtual_packages(&pool, &repo, &problem.virtual_packages)
            .map_err(SolveError::ErrorAddingInstalledPackages)?;
        pool.set_installed(&repo);

        let i = repo_mapping.len();
        repo_mapping.insert(repo.id(), i);
        all_repodata_records.push(installed_records.as_slice());

        // Create datastructures for solving
        pool.create_whatprovides();

        // Add matchspec to the queue
        let mut queue = Queue::default();
        for (spec, request) in problem.specs {
            let id = pool.intern_matchspec(&spec);
            queue.push_id_with_flags(id, SolvableFlags::from(request));
        }

        // Construct a solver and solve the problems in the queue
        let mut solver = pool.create_solver();
        solver.set_flag(SolverFlag::allow_uninstall(), true);
        solver.set_flag(SolverFlag::allow_downgrade(), true);
        if solver.solve(&mut queue).is_err() {
            return Err(SolveError::Unsolvable);
        }

        // Construct a transaction from the solver
        let mut transaction = solver.create_transaction();
        let operations = transaction
            .get_package_operations(&pool, &repo_mapping, &all_repodata_records)
            .map_err(|unsupported_operation_ids| {
                SolveError::UnsupportedOperations(
                    unsupported_operation_ids
                        .into_iter()
                        .map(|id| format!("libsolv operation {id}"))
                        .collect(),
                )
            })?;

        Ok(operations)
    }
}

/// Adds [`RepoDataRecord`] to this instance
pub fn add_repodata_records(
    pool: &Pool,
    repo: &Repo,
    repo_datas: &[RepoDataRecord],
) -> Result<(), NulError> {
    let data = repo.add_repodata();

    // Get all the IDs
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
    let mut package_to_type: HashMap<&str, (PackageExtension, SolvableId)> = HashMap::new();

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

        let solvable = solvable_id.resolve(pool);
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
    }

    repo.internalize();

    Ok(())
}

/// When adding packages, we want to make sure that `.conda` packages have preference over `.tar.bz`
/// packages. For that reason, when adding a solvable we check first if a `.conda` version of the
/// package has already been added, in which case
fn add_or_reuse_solvable<'a>(
    pool: &Pool,
    repo: &Repo,
    data: &Repodata,
    package_to_type: &mut HashMap<&'a str, (PackageExtension, SolvableId)>,
    repo_data: &'a RepoDataRecord,
) -> Option<SolvableId> {
    // TODO: does it make sense that two packages have exactly the same file_name? No

    // Sometimes we can reuse an existing solvable
    if let Some((filename, package_type)) = extract_known_filename_extension(&repo_data.file_name) {
        if let Some(&(other_package_type, old_solvable_id)) = package_to_type.get(filename) {
            match package_type.cmp(&other_package_type) {
                Ordering::Less => {
                    // A previous package that we already stored is actually a package of a better
                    // "type" so we'll just use that instead (.conda > .tar.bz)
                    return None;
                }
                Ordering::Greater => {
                    // A previous package has a worse package "type", we'll reuse the handle but
                    // overwrite its attributes

                    // Update the package to the new type mapping
                    package_to_type.insert(filename, (package_type, old_solvable_id));

                    // Reset and reuse the old solvable
                    reset_solvable(pool, repo, data, old_solvable_id);
                    return Some(old_solvable_id);
                }
                Ordering::Equal => {
                    // They both have the same extension? Keep them both I guess?
                    unimplemented!("found a duplicate package")
                }
            }
        } else {
            let solvable_id = repo.add_solvable();
            package_to_type.insert(filename, (package_type, solvable_id));
            return Some(solvable_id);
        }
    } else {
        tracing::warn!("unknown package extension: {}", &repo_data.file_name);
    }

    Some(repo.add_solvable())
}

fn add_virtual_packages(
    pool: &Pool,
    repo: &Repo,
    packages: &[GenericVirtualPackage],
) -> Result<(), NulError> {
    let data = repo.add_repodata();

    let solvable_buildflavor_id = pool.find_interned_str(SOLVABLE_BUILDFLAVOR).unwrap();

    for package in packages {
        // Create a solvable for the package
        let solvable_id = repo.add_solvable();
        let solvable = solvable_id.resolve(pool);

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

#[derive(Copy, Clone, Ord, PartialEq, PartialOrd, Eq)]
enum PackageExtension {
    TarBz2,
    Conda,
}

/// Given a package filename, extracts the filename and the extension if the extension is a known
/// package extension.
fn extract_known_filename_extension(filename: &str) -> Option<(&str, PackageExtension)> {
    if let Some(filename) = filename.strip_suffix(".conda") {
        Some((filename, PackageExtension::Conda))
    } else {
        filename
            .strip_suffix(".tar.bz2")
            .map(|filename| (filename, PackageExtension::TarBz2))
    }
}
