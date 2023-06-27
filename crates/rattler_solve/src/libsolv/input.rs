//! Contains business logic that loads information into libsolv in order to solve a conda
//! environment

use libsolv_rs::id::RepoId;
use libsolv_rs::id::SolvableId;
use libsolv_rs::pool::Pool;
use rattler_conda_types::package::ArchiveType;
use rattler_conda_types::{GenericVirtualPackage, PackageRecord, RepoDataRecord};
use std::cmp::Ordering;
use std::collections::HashMap;

/// Adds [`RepoDataRecord`] to `repo`
///
/// Panics if the repo does not belong to the pool
pub fn add_repodata_records<'a>(
    pool: &mut Pool<'a>,
    repo_id: RepoId,
    repo_datas: &'a [RepoDataRecord],
) -> Vec<SolvableId> {
    // Keeps a mapping from packages added to the repo to the type and solvable
    let mut package_to_type: HashMap<&str, (ArchiveType, SolvableId)> = HashMap::new();

    let mut solvable_ids = Vec::new();
    for (repo_data_index, repo_data) in repo_datas.iter().enumerate() {
        // Create a solvable for the package
        let solvable_id =
            match add_or_reuse_solvable(pool, repo_id, &mut package_to_type, repo_data) {
                Some(id) => id,
                None => continue,
            };

        // Store the current index so we can retrieve the original repo data record
        // from the final transaction
        pool.resolve_solvable_mut(solvable_id)
            .metadata
            .original_index = Some(repo_data_index);

        let record = &repo_data.package_record;

        // Dependencies
        for match_spec in record.depends.iter() {
            pool.add_dependency(solvable_id, match_spec.to_string());
        }

        // Constrains
        for match_spec in record.constrains.iter() {
            pool.add_constrains(solvable_id, match_spec.to_string());
        }

        solvable_ids.push(solvable_id)
    }

    solvable_ids
}

/// When adding packages, we want to make sure that `.conda` packages have preference over `.tar.bz`
/// packages. For that reason, when adding a solvable we check first if a `.conda` version of the
/// package has already been added, in which case we forgo adding its `.tar.bz` version (and return
/// `None`). If no `.conda` version has been added, we create a new solvable (replacing any existing
/// solvable for the `.tar.bz` version of the package).
fn add_or_reuse_solvable<'a>(
    pool: &mut Pool<'a>,
    repo_id: RepoId,
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
                    pool.reset_package(repo_id, old_solvable_id, &repo_data.package_record);
                    return Some(old_solvable_id);
                }
                Ordering::Equal => {
                    unreachable!("found a duplicate package")
                }
            }
        } else {
            let solvable_id = pool.add_package(repo_id, &repo_data.package_record);
            package_to_type.insert(filename, (archive_type, solvable_id));
            return Some(solvable_id);
        }
    } else {
        tracing::warn!("unknown package extension: {}", &repo_data.file_name);
    }

    let solvable_id = pool.add_package(repo_id, &repo_data.package_record);
    Some(solvable_id)
}

pub fn add_virtual_packages(pool: &mut Pool, repo_id: RepoId, packages: &[GenericVirtualPackage]) {
    let packages: &'static _ = packages
        .iter()
        .map(|p| PackageRecord {
            arch: None,
            name: p.name.clone(),
            noarch: Default::default(),
            platform: None,
            sha256: None,
            size: None,
            subdir: "".to_string(),
            timestamp: None,
            build_number: 0,
            version: p.version.clone(),
            build: p.build_string.clone(),
            depends: Vec::new(),
            features: None,
            legacy_bz2_md5: None,
            legacy_bz2_size: None,
            license: None,
            license_family: None,
            constrains: vec![],
            md5: None,
            track_features: vec![],
        })
        .collect::<Vec<_>>()
        .leak();

    for package in packages {
        pool.add_package(repo_id, &package);
    }
}
