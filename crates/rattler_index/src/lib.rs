//! Indexing of packages in a output folder to create up to date repodata.json
//! files
#![deny(missing_docs)]

use anyhow::Result;
use fs_err::{self as fs, metadata};
use futures::future::try_join_all;
use rattler_conda_types::{
    package::{ArchiveType, IndexJson, PackageFile},
    ChannelInfo, PackageRecord, Platform, RepoData,
};
use rattler_package_streaming::{read, seek};
use std::{
    collections::{HashMap, HashSet},
    future,
    io::Read,
    path::Path,
    str::FromStr,
};

// use fs_err::File;
use opendal::{Configurator, Operator};
// use rattler_conda_types::{
//     package::{ArchiveType, IndexJson, PackageFile},
//     ChannelInfo, PackageRecord, Platform, RepoData,
// };
// use rattler_package_streaming::{read, seek};
// use walkdir::WalkDir;

/// Extract the package record from an `index.json` file.
pub fn package_record_from_index_json<T: Read>(
    file: &Path,
    index_json_reader: &mut T,
) -> std::io::Result<PackageRecord> {
    let index = IndexJson::from_reader(index_json_reader)?;

    let sha256_result = rattler_digest::compute_file_digest::<rattler_digest::Sha256>(file)?;
    let md5_result = rattler_digest::compute_file_digest::<rattler_digest::Md5>(file)?;
    let size = fs::metadata(file)?.len();

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
        extra_depends: std::collections::BTreeMap::new(),
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
pub fn package_record_from_tar_bz2(file: &Path) -> std::io::Result<PackageRecord> {
    let reader = fs::File::open(file)?;
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
pub fn package_record_from_conda(file: &Path) -> std::io::Result<PackageRecord> {
    let reader = fs::File::open(file)?;
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

async fn index_subdir(subdir: Platform, op: Operator, force: bool) -> Result<()> {
    let mut registered_packages = HashMap::default();
    if !force {
        let repodata_path = format!("{}/repodata.json", subdir);
        let repodata_bytes = op.read(&repodata_path).await?;
        let repodata: RepoData = serde_json::from_slice(&repodata_bytes.to_vec())?;
        registered_packages.extend(repodata.packages.into_iter());
    }
    let uploaded_packages: HashSet<String> = op
        .list_with(subdir.as_str())
        .recursive(false)
        .await?
        .into_iter()
        .filter_map(|entry| {
            if entry.metadata().mode().is_file() {
                let filename = entry.name().to_string();
                // Check if the file is an archive package file.
                ArchiveType::try_from(&filename).map(|t| filename)
            } else {
                None
            }
        })
        .collect();

    let packages_to_add = uploaded_packages
        .difference(&registered_packages.keys().cloned().collect::<HashSet<_>>())
        .cloned()
        .collect::<Vec<_>>();

    let packages_to_delete = registered_packages
        .keys()
        .cloned()
        .collect::<HashSet<_>>()
        .difference(&uploaded_packages)
        .cloned()
        .collect::<Vec<_>>();

    let tasks = packages_to_add
        .iter()
        .map(|filename| {
            let op = op.clone();
            let subdir = subdir.clone();
            let filename = filename.clone();
            async move {
                let file_path = format!("{}/{}", subdir, filename);
                // TODO: Check how we use streaming here
                let bytes = op.read(&file_path).await?;
                let archive_type = ArchiveType::try_from(&filename).unwrap();
                // TODO: Rewrite package_record* functions to support streaming
                let record = match archive_type {
                    ArchiveType::TarBz2 => package_record_from_tar_bz2(&file_path),
                    ArchiveType::Conda => package_record_from_conda(&file_path),
                }?;
                registered_packages.insert(filename.clone(), record);
                Ok(())
            }
        })
        .collect::<Vec<_>>();
    try_join_all(tasks).await?;

    for filename in packages_to_delete {
        registered_packages.remove(&filename);
    }

    let repodata = RepoData {
        info: Some(ChannelInfo {
            subdir: subdir.to_string(),
            base_url: None,
        }),
        packages: registered_packages,
        conda_packages: HashMap::default(),
        removed: HashSet::default(),
        version: Some(2),
    };

    let repodata_path = format!("{}/repodata.json", subdir);
    let repodata_bytes = serde_json::to_vec(&repodata)?;
    op.write(&repodata_path, &repodata_bytes.into()).await?;

    Ok(())
}

/// Create a new `repodata.json` for all packages in the given output folder. If
/// `target_platform` is `Some`, only that specific subdir is indexed. Otherwise
/// indexes all subdirs and creates a `repodata.json` for each.
pub async fn index<T: Configurator>(
    target_platform: Option<&Platform>,
    config: T,
    force: bool,
) -> anyhow::Result<()> {
    let builder = config.into_builder();

    // 1. Get all subdirs
    // 2. If not noarch in subdirs, create
    // 3. If target_platform is Some and in subdirs, create
    // 4. For all subdirs
    //      1. Init `registered_packages` HashMap<String, PackageRecord> (key = filename)
    //      if `--force`, we want to set this to empty hashmap
    //      2. Read repodata.json (if exists) and fill `registered_packages`
    //      3. Init `uploaded_packages` HashSet<String> (filenames)
    //      4. For all files in subdir
    //            1. Add filename to `uploaded_packages`
    //      5. `to_add = uploaded_packages - registered_packages.keys()`
    //      6. `to_delete = registered_packages.keys() - uploaded_packages`
    //      7. For all files in `to_add`
    //           1. Stream file
    //           2. Add to `registered_packages`
    //      8. For all files in `to_delete`
    //           1. Remove from `registered_packages`
    //      9. Write `registered_packages` to repodata.json

    // Get all subdirs
    let op = Operator::new(builder)?.finish();
    let entries = op.list_with("").recursive(false).await?;
    let mut subdirs = entries
        .iter()
        .filter_map(|entry| {
            entry.metadata().mode().is_dir().then(|| {
                // Directory entries always end with `/`.
                entry.name().trim_end_matches('/').to_string()
            })
        })
        .map(|s| Platform::from_str(&s))
        .collect::<Result<HashSet<_>, _>>()?;

    // If `noarch` subdir does not exist, we create it.
    if !subdirs.contains(&Platform::NoArch) {
        op.create_dir(Platform::NoArch.as_str()).await?;
        subdirs.insert(Platform::NoArch);
    }

    // If requested `target_platform` subdir does not exist, we create it.
    if let Some(target_platform) = target_platform {
        if !subdirs.contains(&target_platform) {
            op.create_dir(target_platform.as_str()).await?;
            subdirs.insert(*target_platform);
        }
    }

    let tasks = subdirs
        .iter()
        .map(|subdir| index_subdir(*subdir, op.clone(), force))
        .collect::<Vec<_>>();
    try_join_all(tasks).await?;

    // let entries = WalkDir::new(channel_dir).into_iter();
    // let entries: Vec<(PathBuf, ArchiveType)> = entries
    //     .filter_entry(|e| e.depth() <= 2)
    //     .filter_map(Result::ok)
    //     .filter_map(|e| {
    //         ArchiveType::split_str(e.path().to_string_lossy().as_ref())
    //             .map(|(p, t)| (PathBuf::from(format!("{}{}", p, t.extension())), t))
    //     })
    //     .collect();

    // // find all subdirs
    // let mut platforms = entries
    //     .iter()
    //     .filter_map(|(p, _)| {
    //         p.parent().and_then(Path::file_name).and_then(|file_name| {
    //             let name = file_name.to_string_lossy().to_string();
    //             if name == "src_cache" {
    //                 None
    //             } else {
    //                 Some(name)
    //             }
    //         })
    //     })
    //     .collect::<std::collections::HashSet<_>>();

    // // Always create noarch subdir
    // if !channel_dir.join("noarch").exists() {
    //     std::fs::create_dir(channel_dir.join("noarch"))?;
    // }

    // // Make sure that we index noarch if it is not already indexed
    // if !channel_dir.join("noarch/repodata.json").exists() {
    //     platforms.insert("noarch".to_string());
    // }

    // // Create target platform dir if needed
    // if let Some(target_platform) = target_platform {
    //     let platform_str = target_platform.to_string();
    //     if !channel_dir.join(&platform_str).exists() {
    //         std::fs::create_dir(channel_dir.join(&platform_str))?;
    //         platforms.insert(platform_str);
    //     }
    // }

    // for platform in platforms {
    //     if let Some(target_platform) = target_platform {
    //         if platform != target_platform.to_string() {
    //             if platform == "noarch" {
    //                 // check that noarch is already indexed if it is not the target platform
    //                 if channel_dir.join("noarch/repodata.json").exists() {
    //                     continue;
    //                 }
    //             } else {
    //                 continue;
    //             }
    //         }
    //     }

    //     let mut repodata = RepoData {
    //         info: Some(ChannelInfo {
    //             subdir: platform.clone(),
    //             base_url: None,
    //         }),
    //         packages: HashMap::default(),
    //         conda_packages: HashMap::default(),
    //         removed: HashSet::default(),
    //         version: Some(2),
    //     };

    //     for (p, t) in entries.iter().filter_map(|(p, t)| {
    //         p.parent().and_then(|parent| {
    //             parent.file_name().and_then(|file_name| {
    //                 if file_name == OsStr::new(&platform) {
    //                     // If the file_name is the platform we're looking for, return Some((p, t))
    //                     Some((p, t))
    //                 } else {
    //                     // Otherwise, we return None to filter out this item
    //                     None
    //                 }
    //             })
    //         })
    //     }) {
    //         let record = match t {
    //             ArchiveType::TarBz2 => package_record_from_tar_bz2(p),
    //             ArchiveType::Conda => package_record_from_conda(p),
    //         };
    //         let (Ok(record), Some(file_name)) = (record, p.file_name()) else {
    //             tracing::info!("Could not read package record from {:?}", p);
    //             continue;
    //         };
    //         match t {
    //             ArchiveType::TarBz2 => repodata
    //                 .packages
    //                 .insert(file_name.to_string_lossy().to_string(), record),
    //             ArchiveType::Conda => repodata
    //                 .conda_packages
    //                 .insert(file_name.to_string_lossy().to_string(), record),
    //         };
    //     }
    //     let out_file = channel_dir.join(platform).join("repodata.json");
    //     File::create(&out_file)?.write_all(serde_json::to_string_pretty(&repodata)?.as_bytes())?;
    // }

    Ok(())
}

// TODO: write proper unit tests for above functions
