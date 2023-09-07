//! Contains business logic that loads information into libsolv in order to solve a conda
//! environment

use crate::libsolv_rs::{SolverMatchSpec, SolverPackageRecord};
use rattler_conda_types::package::ArchiveType;
use rattler_conda_types::ParseMatchSpecError;
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, RepoDataRecord};
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
    // Iterate over all records and dedup records that refer to the same package data but with
    // different archive types. This can happen if you have two variants of the same package but
    // with different extensions. We prefer `.conda` packages over `.tar.bz`.
    let mut package_to_type: HashMap<&str, (ArchiveType, &'a RepoDataRecord)> = HashMap::new();
    for record in repo_datas.into_iter() {
        let (file_name, archive_type) = ArchiveType::split_str(&record.file_name)
            .unwrap_or((&record.file_name, ArchiveType::TarBz2));
        match package_to_type.get_mut(file_name) {
            None => {
                package_to_type.insert(file_name, (archive_type, record));
            }
            Some((prev_archive_type, prev_record)) => match archive_type.cmp(prev_archive_type) {
                Ordering::Greater => {
                    // A previous package has a worse package "type", we'll use the current record
                    // instead.
                    *prev_archive_type = archive_type;
                    *prev_record = record;
                }
                Ordering::Less => {
                    // A previous package that we already stored is actually a package of a better
                    // "type" so we'll just use that instead (.conda > .tar.bz)
                }
                Ordering::Equal => {
                    if record != *prev_record {
                        unreachable!(
                            "found duplicate record with different values for {}",
                            &record.file_name
                        );
                    }
                }
            },
        }
    }

    let solvable_ids = Vec::new();
    for (_archive_type, repo_data) in package_to_type.into_values() {
        let record = &repo_data.package_record;

        // Add the package to the pool
        let name_id = pool.intern_package_name(record.name.as_normalized());
        let solvable_id =
            pool.add_package(repo_id, name_id, SolverPackageRecord::Record(repo_data));

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
    }

    Ok(solvable_ids)
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
            let version_set_id = pool.intern_version_set(dependency_name, match_spec.into());
            parse_match_spec_cache.insert(spec_str.to_owned(), version_set_id);
            version_set_id
        }
    })
}
