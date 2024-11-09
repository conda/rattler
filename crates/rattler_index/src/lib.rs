//! Indexing of packages in a output folder to create up to date repodata.json
//! files
#![deny(missing_docs)]

use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use fs_err::File;
use rattler_conda_types::{
    package::{ArchiveType, IndexJson, PackageFile},
    ChannelInfo, PackageRecord, Platform, RepoData,
};
use rattler_package_streaming::{read, seek};
use walkdir::WalkDir;

/// Extract the package record from an `index.json` file.
pub fn package_record_from_index_json<T: Read>(
    file: &Path,
    index_json_reader: &mut T,
) -> Result<PackageRecord, std::io::Error> {
    let index = IndexJson::from_reader(index_json_reader)?;

    let sha256_result = rattler_digest::compute_file_digest::<rattler_digest::Sha256>(file)?;
    let md5_result = rattler_digest::compute_file_digest::<rattler_digest::Md5>(file)?;
    let size = std::fs::metadata(file)?.len();

    let package_record = PackageRecord {
        name: index.name,
        version: index.version,
        build: index.build,
        build_number: index.build_number,
        subdir: index.subdir.unwrap_or_else(|| "unknown".to_string()),
        md5: Some(md5_result),
        sha256: Some(sha256_result),
        size: Some(size),
        arch: index.arch,
        platform: index.platform,
        depends: index.depends,
        constrains: index.constrains,
        track_features: index.track_features,
        features: index.features,
        noarch: index.noarch,
        license: index.license,
        license_family: index.license_family,
        timestamp: index.timestamp,
        python_site_packages_path: index.python_site_packages_path,
        legacy_bz2_md5: None,
        legacy_bz2_size: None,
        purls: None,
        run_exports: None,
    };

    Ok(package_record)
}

/// Extract the package record from a `.tar.bz2` package file.
/// This function will look for the `info/index.json` file in the conda package
/// and extract the package record from it.
pub fn package_record_from_tar_bz2(file: &Path) -> Result<PackageRecord, std::io::Error> {
    let reader = std::fs::File::open(file)?;
    let mut archive = read::stream_tar_bz2(reader);
    for entry in archive.entries()?.flatten() {
        let mut entry = entry;
        let path = entry.path()?;
        if path.as_os_str().eq("info/index.json") {
            return package_record_from_index_json(file, &mut entry);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        "No index.json found",
    ))
}

/// Extract the package record from a `.conda` package file.
/// This function will look for the `info/index.json` file in the conda package
/// and extract the package record from it.
pub fn package_record_from_conda(file: &Path) -> Result<PackageRecord, std::io::Error> {
    let reader = std::fs::File::open(file)?;
    let mut archive = seek::stream_conda_info(reader).expect("Could not open conda file");

    for entry in archive.entries()?.flatten() {
        let mut entry = entry;
        let path = entry.path()?;
        if path.as_os_str().eq("info/index.json") {
            return package_record_from_index_json(file, &mut entry);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        "No index.json found",
    ))
}

/// Create a new `repodata.json` for all packages in the given output folder. If
/// `target_platform` is `Some`, only that specific subdir is indexed. Otherwise
/// indexes all subdirs and creates a `repodata.json` for each.
pub fn index(
    output_folder: &Path,
    target_platform: Option<&Platform>,
) -> Result<(), std::io::Error> {
    let entries = WalkDir::new(output_folder).into_iter();
    let entries: Vec<(PathBuf, ArchiveType)> = entries
        .filter_entry(|e| e.depth() <= 2)
        .filter_map(Result::ok)
        .filter_map(|e| {
            ArchiveType::split_str(e.path().to_string_lossy().as_ref())
                .map(|(p, t)| (PathBuf::from(format!("{}{}", p, t.extension())), t))
        })
        .collect();

    // find all subdirs
    let mut platforms = entries
        .iter()
        .filter_map(|(p, _)| {
            p.parent().and_then(Path::file_name).and_then(|file_name| {
                let name = file_name.to_string_lossy().to_string();
                if name == "src_cache" {
                    None
                } else {
                    Some(name)
                }
            })
        })
        .collect::<std::collections::HashSet<_>>();

    // Always create noarch subdir
    if !output_folder.join("noarch").exists() {
        std::fs::create_dir(output_folder.join("noarch"))?;
    }

    // Make sure that we index noarch if it is not already indexed
    if !output_folder.join("noarch/repodata.json").exists() {
        platforms.insert("noarch".to_string());
    }

    // Create target platform dir if needed
    if let Some(target_platform) = target_platform {
        let platform_str = target_platform.to_string();
        if !output_folder.join(&platform_str).exists() {
            std::fs::create_dir(output_folder.join(&platform_str))?;
            platforms.insert(platform_str);
        }
    }

    for platform in platforms {
        if let Some(target_platform) = target_platform {
            if platform != target_platform.to_string() {
                if platform == "noarch" {
                    // check that noarch is already indexed if it is not the target platform
                    if output_folder.join("noarch/repodata.json").exists() {
                        continue;
                    }
                } else {
                    continue;
                }
            }
        }

        let mut repodata = RepoData {
            info: Some(ChannelInfo {
                subdir: platform.clone(),
                base_url: None,
            }),
            packages: HashMap::default(),
            conda_packages: HashMap::default(),
            removed: HashSet::default(),
            version: Some(2),
        };

        for (p, t) in entries.iter().filter_map(|(p, t)| {
            p.parent().and_then(|parent| {
                parent.file_name().and_then(|file_name| {
                    if file_name == OsStr::new(&platform) {
                        // If the file_name is the platform we're looking for, return Some((p, t))
                        Some((p, t))
                    } else {
                        // Otherwise, we return None to filter out this item
                        None
                    }
                })
            })
        }) {
            let record = match t {
                ArchiveType::TarBz2 => package_record_from_tar_bz2(p),
                ArchiveType::Conda => package_record_from_conda(p),
            };
            let (Ok(record), Some(file_name)) = (record, p.file_name()) else {
                tracing::info!("Could not read package record from {:?}", p);
                continue;
            };
            match t {
                ArchiveType::TarBz2 => repodata
                    .packages
                    .insert(file_name.to_string_lossy().to_string(), record),
                ArchiveType::Conda => repodata
                    .conda_packages
                    .insert(file_name.to_string_lossy().to_string(), record),
            };
        }
        let out_file = output_folder.join(platform).join("repodata.json");
        File::create(&out_file)?.write_all(serde_json::to_string_pretty(&repodata)?.as_bytes())?;
    }

    Ok(())
}

// TODO: write proper unit tests for above functions
