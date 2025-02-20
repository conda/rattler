//! Indexing of packages in a output folder to create up to date repodata.json
//! files
#![deny(missing_docs)]

use anyhow::Result;
use fs_err::{self as fs};
use futures::future::try_join_all;
use rattler_conda_types::{
    package::{ArchiveType, IndexJson, PackageFile},
    ChannelInfo, PackageRecord, Platform, RepoData,
};
use rattler_package_streaming::{read, seek};
use std::{
    collections::{HashMap, HashSet},
    io::{Cursor, Read},
    path::Path,
    str::FromStr,
};

use opendal::{Configurator, Operator};

/// Extract the package record from an `index.json` file.
pub fn package_record_from_index_json<T: Read>(
    package_as_bytes: impl AsRef<[u8]>,
    index_json_reader: &mut T,
) -> std::io::Result<PackageRecord> {
    let index = IndexJson::from_reader(index_json_reader)?;

    let sha256_result =
        rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(&package_as_bytes);
    let md5_result = rattler_digest::compute_bytes_digest::<rattler_digest::Md5>(&package_as_bytes);
    let size = package_as_bytes.as_ref().len();

    let package_record = PackageRecord {
        name: index.name,
        version: index.version,
        build: index.build,
        build_number: index.build_number,
        subdir: index.subdir.unwrap_or_else(|| "unknown".to_string()),
        md5: Some(md5_result),
        sha256: Some(sha256_result),
        size: Some(size as u64),
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
    package_record_from_tar_bz2_reader(reader)
}

/// Extract the package record from a `.tar.bz2` package file.
/// This function will look for the `info/index.json` file in the conda package
/// and extract the package record from it.
pub fn package_record_from_tar_bz2_reader(reader: impl Read) -> std::io::Result<PackageRecord> {
    let bytes = reader.bytes().collect::<Result<Vec<u8>, _>>()?;
    let reader = Cursor::new(bytes.clone());
    let mut archive = read::stream_tar_bz2(reader);
    for entry in archive.entries()?.flatten() {
        let mut entry = entry;
        let path = entry.path()?;
        if path.as_os_str().eq("info/index.json") {
            return package_record_from_index_json(bytes, &mut entry);
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
    package_record_from_conda_reader(reader)
}

/// TODO
pub fn package_record_from_conda_reader(reader: impl Read) -> std::io::Result<PackageRecord> {
    let bytes = reader.bytes().collect::<Result<Vec<u8>, _>>()?;
    let reader = Cursor::new(bytes.clone());
    let mut archive = seek::stream_conda_info(reader).expect("Could not open conda file");

    for entry in archive.entries()?.flatten() {
        let mut entry = entry;
        let path = entry.path()?;
        if path.as_os_str().eq("info/index.json") {
            return package_record_from_index_json(bytes, &mut entry);
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
        let repodata_bytes = op.read(&repodata_path).await;
        let repodata: RepoData = match repodata_bytes {
            Ok(bytes) => serde_json::from_slice(&bytes.to_vec())?,
            Err(e) => {
                if e.kind() != opendal::ErrorKind::NotFound {
                    return Err(e.into());
                }
                tracing::info!("Could not find repodata.json. Creating new one.");
                RepoData {
                    info: Some(ChannelInfo {
                        subdir: subdir.to_string(),
                        base_url: None,
                    })
                    .into(),
                    packages: HashMap::default(),
                    conda_packages: HashMap::default(),
                    removed: HashSet::default(),
                    version: Some(2),
                }
            }
        };
        registered_packages.extend(repodata.packages.into_iter());
        tracing::debug!(
            "Found {} already registered packages in {}/repodata.json.",
            registered_packages.len(),
            subdir
        );
    }
    let uploaded_packages: HashSet<String> = op
        .list_with(&format!("{}/", subdir.as_str()))
        .await?
        .iter()
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

    tracing::debug!(
        "Found {} already uploaded packages in subdir {}.",
        uploaded_packages.len(),
        subdir
    );

    let packages_to_delete = registered_packages
        .keys()
        .cloned()
        .collect::<HashSet<_>>()
        .difference(&uploaded_packages)
        .cloned()
        .collect::<Vec<_>>();

    tracing::debug!(
        "Deleting {} packages from subdir {}.",
        packages_to_delete.len(),
        subdir
    );

    for filename in packages_to_delete {
        registered_packages.remove(&filename);
    }

    let packages_to_add = uploaded_packages
        .difference(&registered_packages.keys().cloned().collect::<HashSet<_>>())
        .cloned()
        .collect::<Vec<_>>();

    tracing::debug!(
        "Adding {} packages to subdir {}.",
        packages_to_add.len(),
        subdir
    );

    let tasks = packages_to_add
        .iter()
        .map(|filename| {
            let op = op.clone();
            let subdir = subdir.clone();
            let filename = filename.clone();
            async move {
                let file_path = format!("{}/{}", subdir, filename);
                // TODO: Check how we use streaming here
                let buffer = op.read(&file_path).await?;
                let bytes = buffer.to_vec();
                let cursor = Cursor::new(bytes);
                // We already know it's not None
                let archive_type = ArchiveType::try_from(&filename).unwrap();
                let record = match archive_type {
                    ArchiveType::TarBz2 => package_record_from_tar_bz2_reader(cursor),
                    ArchiveType::Conda => package_record_from_conda_reader(cursor),
                }?;
                Ok::<(String, PackageRecord), std::io::Error>((filename.clone(), record))
            }
        })
        .collect::<Vec<_>>();
    let results = try_join_all(tasks).await?;

    tracing::debug!(
        "Successfully added {} packages to subdir {}.",
        results.len(),
        subdir
    );

    for (filename, record) in results {
        registered_packages.insert(filename, record);
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
    op.write(&repodata_path, repodata_bytes).await?;

    Ok(())
}

/// Create a new `repodata.json` for all packages in the given configurator's root.
/// If `target_platform` is `Some`, only that specific subdir is indexed.
/// Otherwise indexes all subdirs and creates a `repodata.json` for each.
///
/// The process is the following:
/// 1. Get all subdirs
/// 2. If not noarch in subdirs, create
/// 3. If target_platform is Some and in subdirs, create
/// 4. For all subdirs
///      1. Init `registered_packages` HashMap<String, PackageRecord> (key = filename)
///      if `--force`, we want to set this to empty hashmap
///      2. Read repodata.json (if exists) and fill `registered_packages`
///      3. Init `uploaded_packages` HashSet<String> (filenames)
///      4. For all files in subdir
///            1. Add filename to `uploaded_packages`
///      5. `to_add = uploaded_packages - registered_packages.keys()`
///      6. `to_delete = registered_packages.keys() - uploaded_packages`
///      7. For all files in `to_add`
///           1. Stream file
///           2. Add to `registered_packages`
///      8. For all files in `to_delete`
///           1. Remove from `registered_packages`
///      9. Write `registered_packages` to repodata.json
pub async fn index<T: Configurator>(
    target_platform: Option<&Platform>,
    config: T,
    force: bool,
) -> anyhow::Result<()> {
    let builder = config.into_builder();

    // Get all subdirs
    let op = Operator::new(builder)?.finish();
    let entries = op.list_with("").await?;
    let mut subdirs = entries
        .iter()
        .filter_map(|entry| {
            if entry.metadata().mode().is_dir() && entry.name() != "/" {
                // Directory entries always end with `/`.
                Some(entry.name().trim_end_matches('/').to_string())
            } else {
                None
            }
        })
        .map(|s| Platform::from_str(&s))
        .collect::<Result<HashSet<_>, _>>()?;

    // If `noarch` subdir does not exist, we create it.
    if !subdirs.contains(&Platform::NoArch) {
        tracing::debug!("Did not find noarch subdir, creating.");
        op.create_dir(&format!("{}/", Platform::NoArch.as_str()))
            .await?;
        subdirs.insert(Platform::NoArch);
    }

    // If requested `target_platform` subdir does not exist, we create it.
    if let Some(target_platform) = target_platform {
        tracing::debug!("Did not find {target_platform} subdir, creating.");
        if !subdirs.contains(&target_platform) {
            op.create_dir(&format!("{}/", target_platform.as_str()))
                .await?;
            subdirs.insert(*target_platform);
        }
    }

    let tasks = subdirs
        .iter()
        .map(|subdir| index_subdir(*subdir, op.clone(), force))
        .collect::<Vec<_>>();
    try_join_all(tasks).await?;

    Ok(())
}
