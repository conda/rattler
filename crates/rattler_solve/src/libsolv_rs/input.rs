//! Contains business logic that loads information into libsolv in order to solve a conda
//! environment

use crate::libsolv_rs::{SolverMatchSpec, SolverPackageRecord};
use rattler_conda_types::package::ArchiveType;
use rattler_conda_types::{NamelessMatchSpec, MatchSpec, ParseMatchSpecError};
use rattler_conda_types::{GenericVirtualPackage, RepoDataRecord};
use rattler_libsolv_rs::{Pool, RepoId, SolvableId, VersionSetId};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::str::FromStr;

// TODO: could abstract away methods adding packages and virtual packages
// to pool

/// Adds [`RepoDataRecord`] to `repo`
///
/// Panics if the repo does not belong to the pool
pub(super) fn add_repodata_records<'a>(
    pool: &mut Pool<SolverMatchSpec<'a>>,
    repo_id: RepoId,
    repo_datas: impl IntoIterator<Item = &'a RepoDataRecord>,
    parse_match_spec_cache: &mut HashMap<String, VersionSetId>,
) -> Result<Vec<SolvableId>, ParseMatchSpecError> {
    // Keeps a mapping from packages added to the repo to the type and solvable
    let mut package_to_type: HashMap<&str, (ArchiveType, SolvableId)> = HashMap::new();

    let mut solvable_ids = Vec::new();
    for repo_data in repo_datas.into_iter() {
        // Create a solvable for the package
        let solvable_id =
            match add_or_reuse_solvable(pool, repo_id, &mut package_to_type, repo_data) {
                Some(id) => id,
                None => continue,
            };

        let record = &repo_data.package_record;

        // Dependencies
        for match_spec_str in record.depends.iter() {
            let version_set_id = parse_match_spec(pool, match_spec_str, parse_match_spec_cache)?;
            pool.add_dependency(solvable_id, version_set_id);
        }

        // Constrains
        for match_spec_str in record.constrains.iter() {
            let version_set_id = parse_match_spec(pool, match_spec_str, parse_match_spec_cache)?;
            pool.add_constrains(solvable_id, version_set_id);
        }

        solvable_ids.push(solvable_id)
    }

    Ok(solvable_ids)
}

/// When adding packages, we want to make sure that `.conda` packages have preference over `.tar.bz`
/// packages. For that reason, when adding a solvable we check first if a `.conda` version of the
/// package has already been added, in which case we forgo adding its `.tar.bz` version (and return
/// `None`). If no `.conda` version has been added, we create a new solvable (replacing any existing
/// solvable for the `.tar.bz` version of the package).
fn add_or_reuse_solvable<'a>(
    pool: &mut Pool<SolverMatchSpec<'a>>,
    repo_id: RepoId,
    package_to_type: &mut HashMap<&'a str, (ArchiveType, SolvableId)>,
    repo_data: &'a RepoDataRecord,
) -> Option<SolvableId> {
    // Resolve the name in the pool
    let package_name_id = pool.intern_package_name(repo_data.package_record.name.as_normalized());

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

                    // Reuse the old solvable
                    pool.overwrite_package(
                        repo_id,
                        old_solvable_id,
                        package_name_id,
                        SolverPackageRecord::Record(repo_data),
                    );
                    return Some(old_solvable_id);
                }
                Ordering::Equal => {
                    unreachable!("found a duplicate package")
                }
            }
        } else {
            let solvable_id = pool.add_package(
                repo_id,
                package_name_id,
                SolverPackageRecord::Record(repo_data),
            );
            package_to_type.insert(filename, (archive_type, solvable_id));
            return Some(solvable_id);
        }
    } else {
        tracing::warn!("unknown package extension: {}", &repo_data.file_name);
    }

    let solvable_id = pool.add_package(
        repo_id,
        package_name_id,
        SolverPackageRecord::Record(repo_data),
    );
    Some(solvable_id)
}

pub(super) fn add_virtual_packages<'a>(
    pool: &mut Pool<SolverMatchSpec<'a>>,
    repo_id: RepoId,
    packages: &'a [GenericVirtualPackage],
) {
    for package in packages {
        let package_name_id = pool.intern_package_name(package.name.as_normalized());
        pool.add_package(
            repo_id,
            package_name_id,
            SolverPackageRecord::VirtualPackage(package),
        );
    }
}

pub(super) fn parse_match_spec(
    pool: &mut Pool<SolverMatchSpec>,
    spec_str: &str,
    parse_match_spec_cache: &mut HashMap<String, VersionSetId>,
) -> Result<VersionSetId, ParseMatchSpecError> {
    Ok(match parse_match_spec_cache.get(spec_str) {
        Some(spec_id) => *spec_id,
        None => {
            let match_spec = MatchSpec::from_str(spec_str)?;
            let dependency_name = pool.intern_package_name(
                match_spec
                    .name
                    .as_ref()
                    .expect("match specs without names are not supported")
                    .as_normalized(),
            );
            let version_set_id = pool.intern_version_set(dependency_name, NamelessMatchSpec::from(match_spec).into());
            parse_match_spec_cache.insert(spec_str.to_owned(), version_set_id);
            version_set_id
        }
    })
}
