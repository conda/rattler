//! Indexing of packages in a output folder to create up to date repodata.json
//! files
#![deny(missing_docs)]

pub mod cache;
mod utils;

use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Cursor, Read, Seek},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::SystemTime,
};

use anyhow::{Context, Result};
use bytes::buf::Buf;
use chrono::{DateTime, Utc};
use fs_err::{self as fs};
use futures::{stream::FuturesUnordered, StreamExt};
use fxhash::FxHashMap;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use opendal::{
    layers::RetryLayer,
    services::{FsConfig, S3Config},
    Configurator, ErrorKind, Operator,
};
use rattler_conda_types::{
    package::{ArchiveIdentifier, ArchiveType, IndexJson, PackageFile, RunExportsJson},
    ChannelInfo, PackageRecord, PatchInstructions, Platform, RepoData, Shard, ShardedRepodata,
    ShardedSubdirInfo,
};
use rattler_digest::Sha256Hash;
use rattler_package_streaming::{
    read,
    seek::{self, stream_conda_content},
};
use rattler_s3::ResolvedS3Credentials;
use retry_policies::{policies::ExponentialBackoff, Jitter, RetryDecision, RetryPolicy};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tracing::Instrument;
use url::Url;

/// Statistics for a single subdir indexing operation
#[derive(Debug, Clone, Default)]
pub struct SubdirIndexStats {
    /// Number of packages added to the index
    pub packages_added: usize,
    /// Number of packages removed from the index
    pub packages_removed: usize,
    /// Number of retries due to concurrent modifications
    pub retries: usize,
}

/// Statistics for the entire indexing operation
#[derive(Debug, Clone, Default)]
pub struct IndexStats {
    /// Statistics per subdir
    pub subdirs: HashMap<Platform, SubdirIndexStats>,
}

const REPODATA_FROM_PACKAGES: &str = "repodata_from_packages.json";
const REPODATA: &str = "repodata.json";
const REPODATA_SHARDS: &str = "repodata_shards.msgpack.zst";
const ZSTD_REPODATA_COMPRESSION_LEVEL: i32 = 19;
const CACHE_CONTROL_IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// Returns a retry policy optimized for write operations with potential lock contention.
///
/// This policy retries for approximately 5 minutes with longer backoff durations compared
/// to the default policy. The backoff progression is:
/// Retries for up to 10 minutes total, with delays between retries starting at 10 seconds and
/// capping at 90 seconds, and applying bounded jitter to avoid thundering herd issues.
///
/// This is designed for scenarios where multiple processes may be writing to the same
/// resource and need to wait for locks to be released, such as concurrent repodata
/// indexing operations.
pub fn write_retry_policy() -> impl RetryPolicy {
    ExponentialBackoff::builder()
        .retry_bounds(
            std::time::Duration::from_secs(10), // min delay: 10 seconds
            std::time::Duration::from_secs(90), // max delay: 90 seconds
        )
        .jitter(Jitter::Bounded)
        .build_with_total_retry_duration(std::time::Duration::from_secs(600)) // Retry for up to 10 minutes total
}

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
        experimental_extra_depends: index.experimental_extra_depends,
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
        purls: index.purls,
        run_exports: None,
    };

    Ok(package_record)
}

fn repodata_patch_from_conda_package_stream<'a>(
    package: impl Read + Seek + 'a,
) -> anyhow::Result<rattler_conda_types::RepoDataPatch> {
    let mut subdirs = FxHashMap::default();

    let mut content_reader = stream_conda_content(package)?;
    let entries = content_reader.entries()?;
    for entry in entries {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            return Err(anyhow::anyhow!(
                "Expected repodata patch package to be a file"
            ));
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        let path = entry.path()?;
        let components = path.components().collect::<Vec<_>>();
        let subdir =
            if components.len() == 2 && components[1].as_os_str() == "patch_instructions.json" {
                let subdir_str = components[0]
                    .as_os_str()
                    .to_str()
                    .context("Could not convert OsStr to str")?;
                let _ = Platform::from_str(subdir_str)?;
                subdir_str.to_string()
            } else {
                return Err(anyhow::anyhow!(
                    "Expected files of form <subdir>/patch_instructions.json, but found {}",
                    path.display()
                ));
            };

        let instructions: PatchInstructions = serde_json::from_slice(&buf)?;
        subdirs.insert(subdir, instructions);
    }

    Ok(rattler_conda_types::RepoDataPatch { subdirs })
}

/// Extract the package record from a `.tar.bz2` package file.
/// This function will look for the `info/index.json` file in the conda package
/// and extract the package record from it.
pub fn package_record_from_tar_bz2(file: &Path) -> std::io::Result<PackageRecord> {
    let reader = fs::File::open(file)?;
    package_record_from_tar_bz2_reader(BufReader::new(reader))
}

/// Extract the package record from a `.tar.bz2` package file.
/// This function will look for the `info/index.json` file in the conda package
/// and extract the package record from it.
pub fn package_record_from_tar_bz2_reader(reader: impl BufRead) -> std::io::Result<PackageRecord> {
    let bytes = reader.bytes().collect::<Result<Vec<u8>, _>>()?;
    let reader = Cursor::new(&bytes);
    let mut archive = read::stream_tar_bz2(reader);
    for entry in archive.entries()?.flatten() {
        let mut entry = entry;
        let path = entry.path()?;
        if path.as_os_str().eq("info/index.json") {
            return package_record_from_index_json(&bytes, &mut entry);
        }
    }
    Err(std::io::Error::other("No index.json found"))
}

/// Extract the package record from a `.conda` package file.
/// This function will look for the `info/index.json` file in the conda package
/// and extract the package record from it.
pub fn package_record_from_conda(file: &Path) -> std::io::Result<PackageRecord> {
    let reader = fs::File::open(file)?;
    package_record_from_conda_reader(BufReader::new(reader))
}

fn read_index_json_from_archive(
    bytes: &Vec<u8>,
    archive: &mut tar::Archive<impl Read>,
) -> std::io::Result<PackageRecord> {
    let mut index_json = None;
    let mut run_exports_json = None;
    for entry in archive.entries()?.flatten() {
        let mut entry = entry;
        let path = entry.path()?;
        if path.as_os_str().eq("info/index.json") {
            index_json = Some(package_record_from_index_json(bytes, &mut entry)?);
        } else if path.as_os_str().eq("info/run_exports.json") {
            run_exports_json = Some(RunExportsJson::from_reader(&mut entry)?);
        }
    }

    if let Some(mut index_json) = index_json {
        index_json.run_exports = run_exports_json;
        return Ok(index_json);
    }

    Err(std::io::Error::other("No index.json found"))
}

/// Extract the package record from a `.conda` package file content.
/// This function will look for the `info/index.json` file in the conda package
/// and extract the package record from it.
pub fn package_record_from_conda_reader(reader: impl BufRead) -> std::io::Result<PackageRecord> {
    let bytes = reader.bytes().collect::<Result<Vec<u8>, _>>()?;
    let reader = Cursor::new(&bytes);
    let mut archive = seek::stream_conda_info(reader).expect("Could not open conda file");
    read_index_json_from_archive(&bytes, &mut archive)
}

/// Parse a package file buffer based on its filename extension.
///
/// # Arguments
///
/// * `buffer` - The file contents to parse
/// * `filename` - The filename (used to determine archive type)
///
/// # Returns
///
/// Returns the parsed `PackageRecord`.
fn parse_package_buffer(buffer: opendal::Buffer, filename: &str) -> std::io::Result<PackageRecord> {
    let reader = buffer.reader();
    let archive_type = ArchiveType::try_from(filename).unwrap();
    match archive_type {
        ArchiveType::TarBz2 => package_record_from_tar_bz2_reader(reader),
        ArchiveType::Conda => package_record_from_conda_reader(reader),
    }
}

/// Read and parse a package file with caching and retry logic.
///
/// This function encapsulates the logic for reading a package file, including:
/// - Checking the cache for a previously computed record
/// - Reading the file with retry logic on cache miss
/// - Parsing the package content
/// - Storing the result in the cache
///
/// # Arguments
///
/// * `op` - The operator to use for file operations
/// * `cache` - The package record cache (scoped to a single subdir)
/// * `subdir` - The subdirectory (e.g., "noarch", "linux-64")
/// * `filename` - The package filename (e.g., "package-1.0.0.tar.bz2")
///
/// # Returns
///
/// Returns the parsed `PackageRecord` on success.
async fn read_and_parse_package(
    op: &Operator,
    cache: &cache::PackageRecordCache,
    subdir: Platform,
    filename: &str,
) -> std::io::Result<PackageRecord> {
    let file_path = format!("{subdir}/{filename}");

    // Try cache or get current metadata
    // Cache uses filename as key since it's scoped to a single subdir
    match cache.get_or_stat(op, &file_path).await {
        Ok(cache::CacheResult::Hit(record)) => {
            // Cache hit - reuse the record
            Ok(*record)
        }
        Ok(cache::CacheResult::Miss {
            etag,
            last_modified,
        }) => {
            // Cache miss - read file with retry logic
            let (buffer, final_metadata) = cache::read_package_with_retry(
                op,
                &file_path,
                RepodataFileMetadata {
                    etag,
                    last_modified,
                },
            )
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

            // Parse package
            let record = parse_package_buffer(buffer, filename)?;

            // Store in cache using filename as key
            cache
                .insert(
                    &file_path,
                    record.clone(),
                    final_metadata.etag,
                    final_metadata.last_modified,
                )
                .await;

            Ok(record)
        }
        Err(e) => {
            tracing::warn!("Cache stat failed for {file_path}: {e}, proceeding without cache");
            // Fall back to direct read without cache
            let buffer = op
                .read(&file_path)
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            parse_package_buffer(buffer, filename)
        }
    }
}

/// Metadata for a single repodata file, used to detect concurrent
/// modifications.
#[derive(Debug, Clone)]
pub struct RepodataFileMetadata {
    /// The `ETag` of the file, if available
    pub etag: Option<String>,
    /// The last modified timestamp of the file, if available
    pub last_modified: Option<DateTime<Utc>>,
}

impl RepodataFileMetadata {
    /// Collect metadata for a file without reading its contents.
    /// Returns metadata with None values if the file doesn't exist.
    pub async fn new(op: &Operator, path: &str) -> opendal::Result<Self> {
        match op.stat(path).await {
            Ok(metadata) => Ok(Self {
                etag: metadata.etag().map(str::to_owned),
                last_modified: metadata.last_modified(),
            }),
            Err(e) if e.kind() == opendal::ErrorKind::NotFound => Ok(Self {
                etag: None,
                last_modified: None,
            }),
            Err(e) => Err(e),
        }
    }
}

/// Collection of metadata for all critical repodata files that need concurrent
/// access protection.
#[derive(Debug, Clone)]
pub struct RepodataMetadataCollection {
    /// Metadata for repodata.json
    pub repodata: RepodataFileMetadata,
    /// Metadata for `repodata_from_packages.json` (only when patches are used)
    pub repodata_from_packages: Option<RepodataFileMetadata>,
    /// Metadata for repodata.json.zst
    pub repodata_zst: Option<RepodataFileMetadata>,
    /// Metadata for `repodata_shards.msgpack.zst`
    pub repodata_shards: Option<RepodataFileMetadata>,
}

impl RepodataMetadataCollection {
    /// Collect metadata for all critical repodata files in a subdir.
    pub async fn new(
        op: &Operator,
        subdir: Platform,
        has_patch: bool,
        write_zst: bool,
        write_shards: bool,
    ) -> opendal::Result<Self> {
        // Always track repodata.json
        let repodata = RepodataFileMetadata::new(op, &format!("{subdir}/{REPODATA}")).await?;

        // Track repodata_from_packages.json if patches are used
        let repodata_from_packages = if has_patch {
            Some(
                RepodataFileMetadata::new(op, &format!("{subdir}/{REPODATA_FROM_PACKAGES}"))
                    .await?,
            )
        } else {
            None
        };

        let repodata_zst = if write_zst {
            Some(RepodataFileMetadata::new(op, &format!("{subdir}/{REPODATA}.zst")).await?)
        } else {
            None
        };

        let repodata_shards = if write_shards {
            Some(RepodataFileMetadata::new(op, &format!("{subdir}/{REPODATA_SHARDS}")).await?)
        } else {
            None
        };

        Ok(Self {
            repodata,
            repodata_from_packages,
            repodata_zst,
            repodata_shards,
        })
    }
}

#[allow(clippy::too_many_arguments)]
async fn index_subdir(
    subdir: Platform,
    op: Operator,
    force: bool,
    write_zst: bool,
    write_shards: bool,
    repodata_patch: Option<PatchInstructions>,
    progress: Option<MultiProgress>,
    semaphore: Arc<Semaphore>,
    cache: cache::PackageRecordCache,
) -> Result<SubdirIndexStats> {
    // Use write_retry_policy for handling lock contention during repodata writes
    // This will retry for ~5 minutes with longer backoff durations (10s, 30s, 60s, etc.)
    let retry_policy = write_retry_policy();
    let mut current_try = 0;

    loop {
        let request_start_time = SystemTime::now();

        match index_subdir_inner(
            subdir,
            op.clone(),
            force,
            write_zst,
            write_shards,
            repodata_patch.clone(),
            progress.clone(),
            semaphore.clone(),
            cache.clone(),
        )
        .await
        {
            Ok(mut stats) => {
                stats.retries = current_try;
                return Ok(stats);
            }
            Err(e) => {
                // Check if this is a race condition error
                if let Some(opendal_err) = e.downcast_ref::<opendal::Error>() {
                    if opendal_err.kind() == opendal::ErrorKind::ConditionNotMatch {
                        // Race condition detected - should we retry?
                        match retry_policy.should_retry(request_start_time, current_try as u32) {
                            RetryDecision::Retry { execute_after } => {
                                let duration = execute_after
                                    .duration_since(SystemTime::now())
                                    .unwrap_or_default();
                                tracing::warn!(
                                    "Detected concurrent modification of repodata for {}, retrying in {:?}",
                                    subdir,
                                    duration
                                );
                                tokio::time::sleep(duration).await;
                                current_try += 1;
                                continue;
                            }
                            RetryDecision::DoNotRetry => {
                                tracing::error!(
                                    "Max retries exceeded for {} due to concurrent modifications",
                                    subdir
                                );
                                return Err(e);
                            }
                        }
                    }
                }
                // Not a race condition error, or downcast failed - propagate immediately
                return Err(e);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn index_subdir_inner(
    subdir: Platform,
    op: Operator,
    force: bool,
    write_zst: bool,
    write_shards: bool,
    repodata_patch: Option<PatchInstructions>,
    progress: Option<MultiProgress>,
    semaphore: Arc<Semaphore>,
    cache: cache::PackageRecordCache,
) -> Result<SubdirIndexStats> {
    // Step 1: Collect ETags/metadata for all critical files upfront
    let metadata = RepodataMetadataCollection::new(
        &op,
        subdir,
        repodata_patch.is_some(),
        write_zst,
        write_shards,
    )
    .await?;

    // Step 2: Read any previous repodata.json files with conditional check.
    // This file already contains a lot of information about the packages that we
    // can reuse.
    let mut registered_packages: FxHashMap<String, PackageRecord> = if force {
        HashMap::default()
    } else {
        let (repodata_path, read_metadata) = if repodata_patch.is_some() {
            (
                format!("{subdir}/{REPODATA_FROM_PACKAGES}"),
                metadata.repodata_from_packages.as_ref().unwrap(),
            )
        } else {
            (format!("{subdir}/{REPODATA}"), &metadata.repodata)
        };

        match crate::utils::read_with_metadata_check(&op, &repodata_path, read_metadata).await {
            Ok(bytes) => match serde_json::from_slice::<RepoData>(&bytes.to_vec()) {
                Ok(repodata) => repodata
                    .packages
                    .into_iter()
                    .chain(repodata.conda_packages)
                    .collect(),
                Err(err) => {
                    tracing::warn!("Failed to parse {repodata_path}: {err}. Not reusing content from this file");
                    HashMap::default()
                }
            },
            Err(err) if err.kind() == opendal::ErrorKind::NotFound => {
                tracing::info!("Could not find {repodata_path}. Creating new one.");
                HashMap::default()
            }
            Err(err) => return Err(err.into()),
        }
    };

    // List all the packages in the subdirectory.
    let uploaded_packages: HashSet<String> = op
        .list_with(&format!("{}/", subdir.as_str()))
        .await?
        .iter()
        .filter_map(|entry| {
            if entry.metadata().mode().is_file() {
                let filename = entry.name().to_string();
                // Check if the file is an archive package file.
                ArchiveType::try_from(&filename).map(|_| filename)
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

    // Find packages that are listed in the previous repodata.json file but have
    // since been removed.
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

    let packages_to_add = uploaded_packages
        .difference(&registered_packages.keys().cloned().collect::<HashSet<_>>())
        .cloned()
        .collect::<Vec<_>>();

    tracing::info!(
        "Adding {} packages to subdir {}.",
        packages_to_add.len(),
        subdir
    );

    let pb = if let Some(progress) = progress {
        progress.add(ProgressBar::new(packages_to_add.len() as u64))
    } else {
        ProgressBar::hidden()
    };

    let sty = ProgressStyle::with_template(
        "[{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} {msg}",
    )
    .unwrap()
    .progress_chars("##-");
    pb.set_style(sty);

    let mut tasks = FuturesUnordered::new();
    for filename in packages_to_add.iter() {
        let task = {
            let op = op.clone();
            let filename = filename.clone();
            let pb = pb.clone();
            let semaphore = semaphore.clone();
            let cache = cache.clone();
            async move {
                let _permit = semaphore
                    .acquire()
                    .await
                    .expect("Semaphore was unexpectedly closed");
                pb.set_message(format!(
                    "Indexing {} {}",
                    subdir.as_str(),
                    console::style(&filename).dim()
                ));

                let record = read_and_parse_package(&op, &cache, subdir, &filename).await?;

                pb.inc(1);
                Ok::<(String, PackageRecord), std::io::Error>((filename, record))
            }
        };
        tasks.push(tokio::spawn(task));
    }
    let mut results = Vec::new();
    while let Some(join_result) = tasks.next().await {
        match join_result {
            Ok(Ok(result)) => results.push(result),
            Ok(Err(e)) => {
                tasks.clear();
                tracing::error!("Failed to process package: {}", e);
                pb.abandon_with_message(format!(
                    "{} {}",
                    console::style("Failed to index").red(),
                    console::style(subdir.as_str()).dim()
                ));
                return Err(e.into());
            }
            Err(join_err) => {
                tasks.clear();
                tracing::error!("Task panicked: {}", join_err);
                pb.abandon_with_message(format!(
                    "{} {}",
                    console::style("Failed to index").red(),
                    console::style(subdir.as_str()).dim()
                ));
                return Err(anyhow::anyhow!("Task panicked: {join_err}"));
            }
        }
    }
    pb.finish_with_message(format!(
        "{} {}",
        console::style("Finished").green(),
        subdir.as_str()
    ));

    tracing::info!(
        "Successfully added {} packages to subdir {}.",
        results.len(),
        subdir
    );

    for (filename, record) in results {
        registered_packages.insert(filename, record);
    }

    let mut packages: FxHashMap<String, PackageRecord> = HashMap::default();
    let mut conda_packages: FxHashMap<String, PackageRecord> = HashMap::default();
    for (filename, package) in registered_packages {
        match ArchiveType::try_from(&filename) {
            Some(ArchiveType::TarBz2) => {
                packages.insert(filename, package);
            }
            Some(ArchiveType::Conda) => {
                conda_packages.insert(filename, package);
            }
            _ => panic!("Unknown archive type"),
        }
    }

    // TODO: don't serialize run_exports and purls but in their own files
    let repodata_before_patches = RepoData {
        info: Some(ChannelInfo {
            subdir: Some(subdir.to_string()),
            base_url: None,
        }),
        packages,
        conda_packages,
        removed: HashSet::default(),
        version: Some(2),
    };

    write_repodata(
        repodata_before_patches,
        repodata_patch,
        subdir,
        op,
        &metadata,
    )
    .await?;

    Ok(SubdirIndexStats {
        packages_added: packages_to_add.len(),
        packages_removed: packages_to_delete.len(),
        retries: 0, // Will be set by index_subdir
    })
}

fn serialize_msgpack_zst<T>(val: &T) -> Result<Vec<u8>>
where
    T: Serialize + ?Sized,
{
    let msgpack = rmp_serde::to_vec_named(val)?;
    let encoded = zstd::stream::encode_all(&msgpack[..], 0)?;
    Ok(encoded)
}

/// Write a `repodata.json` for all packages in the given configurator's root.
/// Uses conditional writes based on the provided metadata to prevent concurrent
/// modification issues.
pub async fn write_repodata(
    repodata: RepoData,
    repodata_patch: Option<PatchInstructions>,
    subdir: Platform,
    op: Operator,
    metadata: &RepodataMetadataCollection,
) -> Result<()> {
    if let Some(repodata_from_packages_metadata) = &metadata.repodata_from_packages {
        let unpatched_repodata_path = format!("{subdir}/{REPODATA_FROM_PACKAGES}");
        tracing::info!("Writing unpatched repodata to {unpatched_repodata_path}");
        let unpatched_repodata_bytes = serde_json::to_vec(&repodata)?;
        crate::utils::write_with_metadata_check(
            &op,
            &unpatched_repodata_path,
            unpatched_repodata_bytes,
            repodata_from_packages_metadata,
        )
        .await?;
    }

    let repodata = if let Some(instructions) = repodata_patch {
        tracing::info!("Patching repodata");
        let mut patched_repodata = repodata.clone();
        patched_repodata.apply_patches(&instructions);
        patched_repodata
    } else {
        repodata
    };

    let repodata_bytes = serde_json::to_vec(&repodata)?;

    // Write compressed version if requested
    if let Some(repodata_zst_metadata) = &metadata.repodata_zst {
        tracing::info!("Compressing repodata bytes");
        let repodata_zst_bytes =
            zstd::stream::encode_all(&repodata_bytes[..], ZSTD_REPODATA_COMPRESSION_LEVEL)?;
        let repodata_zst_path = format!("{subdir}/{REPODATA}.zst");
        tracing::info!("Writing zst repodata to {repodata_zst_path}");
        crate::utils::write_with_metadata_check(
            &op,
            &repodata_zst_path,
            repodata_zst_bytes,
            repodata_zst_metadata,
        )
        .await?;
    }

    // Write main repodata.json with conditional check
    let repodata_path = format!("{subdir}/{REPODATA}");
    tracing::info!("Writing repodata to {repodata_path}");
    crate::utils::write_with_metadata_check(
        &op,
        &repodata_path,
        repodata_bytes,
        &metadata.repodata,
    )
    .await?;

    if metadata.repodata_shards.is_some() {
        // See CEP 16 <https://github.com/conda/ceps/blob/main/cep-0016.md>
        tracing::info!("Creating sharded repodata");
        let mut shards_by_package_names: HashMap<String, Shard> = HashMap::new();
        for (k, package_record) in repodata.conda_packages {
            let package_name = package_record.name.as_normalized();
            let shard = shards_by_package_names
                .entry(package_name.into())
                .or_default();
            shard.conda_packages.insert(k, package_record);
        }
        for (k, package_record) in repodata.packages {
            let package_name = package_record.name.as_normalized();
            let shard = shards_by_package_names
                .entry(package_name.into())
                .or_default();
            shard.packages.insert(k, package_record);
        }
        for package in repodata.removed {
            let package_name = ArchiveIdentifier::try_from_filename(package.as_str())
                .context("Could not determine archive identifier for {package}")?
                .name;
            let shard = shards_by_package_names.entry(package_name).or_default();
            shard.removed.insert(package);
        }

        // calculate digests for shards
        let shards = shards_by_package_names
            .iter()
            .map(|(k, shard)| {
                serialize_msgpack_zst(shard).map(|encoded| {
                    let mut hasher = Sha256::new();
                    hasher.update(&encoded);
                    let digest: Sha256Hash = hasher.finalize();
                    (k, (digest, encoded))
                })
            })
            .collect::<Result<HashMap<_, _>>>()?;

        let sharded_repodata = ShardedRepodata {
            info: ShardedSubdirInfo {
                subdir: subdir.to_string(),
                base_url: "".into(),
                shards_base_url: "./shards/".into(),
                created_at: Some(chrono::Utc::now()),
            },
            shards: shards
                .iter()
                .map(|(&k, (digest, _))| (k.clone(), *digest))
                .collect(),
        };

        let mut tasks = FuturesUnordered::new();
        // todo max parallel
        for (_, (digest, encoded_shard)) in shards {
            let op = op.clone();
            let future = async move || {
                let shard_path = format!("{subdir}/shards/{digest:x}.msgpack.zst");
                tracing::trace!("Writing repodata shard to {shard_path}");
                match op
                    .write_with(&shard_path, encoded_shard)
                    .if_not_exists(true)
                    .cache_control(CACHE_CONTROL_IMMUTABLE)
                    .await
                {
                    Err(e) if e.kind() == ErrorKind::ConditionNotMatch => {
                        tracing::trace!("{shard_path} already exists");
                        Ok(())
                    }
                    Ok(_metadata) => Ok(()),
                    Err(e) => Err(e),
                }
            };
            tasks.push(tokio::spawn(future()));
        }
        while let Some(join_result) = tasks.next().await {
            match join_result {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => Err(e)?,
                Err(join_err) => Err(join_err)?,
            }
        }

        // Write sharded repodata index with conditional check
        if let Some(repodata_shards_metadata) = &metadata.repodata_shards {
            let repodata_shards_path = format!("{subdir}/{REPODATA_SHARDS}");
            tracing::trace!("Writing repodata shards to {repodata_shards_path}");
            let sharded_repodata_encoded = serialize_msgpack_zst(&sharded_repodata)?;
            crate::utils::write_with_metadata_check(
                &op,
                &repodata_shards_path,
                sharded_repodata_encoded,
                repodata_shards_metadata,
            )
            .await?;
        }
    }
    Ok(())
}

/// Configuration for `index_fs`
pub struct IndexFsConfig {
    /// The channel to index.
    pub channel: PathBuf,
    /// The target platform to index.
    pub target_platform: Option<Platform>,
    /// The path to a repodata patch to apply to the index.
    pub repodata_patch: Option<String>,
    /// Whether to write the repodata as a zstd-compressed file.
    pub write_zst: bool,
    /// Whether to write the repodata shards.
    pub write_shards: bool,
    /// Whether to force the index to be written.
    pub force: bool,
    /// The maximum number of parallel tasks to run.
    pub max_parallel: usize,
    /// The multi-progress bar to use for the index.
    pub multi_progress: Option<MultiProgress>,
}

/// Create a new `repodata.json` for all packages in the channel at the given
/// directory.
pub async fn index_fs(
    IndexFsConfig {
        channel,
        target_platform,
        repodata_patch,
        write_zst,
        write_shards,
        force,
        max_parallel,
        multi_progress,
    }: IndexFsConfig,
) -> anyhow::Result<()> {
    let mut config = FsConfig::default();
    config.root = Some(channel.canonicalize()?.to_string_lossy().to_string());
    let builder = config.into_builder();
    let op = Operator::new(builder)?.finish();
    index(
        target_platform,
        op,
        repodata_patch,
        write_zst,
        write_shards,
        force,
        max_parallel,
        multi_progress,
    )
    .await
    .map(|_| ())
}

/// Configuration for `index_s3`
pub struct IndexS3Config {
    /// The channel to index.
    pub channel: Url,
    /// The resolved credentials to use for S3 access.
    pub credentials: ResolvedS3Credentials,
    /// The target platform to index.
    pub target_platform: Option<Platform>,
    /// The path to a repodata patch to apply to the index.
    pub repodata_patch: Option<String>,
    /// Whether to write the repodata as a zstd-compressed file.
    pub write_zst: bool,
    /// Whether to write the repodata shards.
    pub write_shards: bool,
    /// Whether to force the index to be written.
    pub force: bool,
    /// The maximum number of parallel tasks to run.
    pub max_parallel: usize,
    /// The multi-progress bar to use for the index.
    pub multi_progress: Option<MultiProgress>,
}

/// Create a new `repodata.json` for all packages in the channel at the given S3
/// URL.
pub async fn index_s3(
    IndexS3Config {
        channel,
        credentials,
        target_platform,
        repodata_patch,
        write_zst,
        write_shards,
        force,
        max_parallel,
        multi_progress,
    }: IndexS3Config,
) -> anyhow::Result<()> {
    // Create the S3 configuration for opendal.
    let mut s3_config = S3Config::default();
    s3_config.root = Some(channel.path().to_string());
    s3_config.bucket = channel
        .host_str()
        .ok_or(anyhow::anyhow!("No bucket in S3 URL"))?
        .to_string();

    s3_config.region = Some(credentials.region);
    s3_config.endpoint = Some(credentials.endpoint_url.to_string());
    s3_config.secret_access_key = Some(credentials.secret_access_key);
    s3_config.access_key_id = Some(credentials.access_key_id);
    s3_config.session_token = credentials.session_token;
    s3_config.enable_virtual_host_style =
        credentials.addressing_style == rattler_s3::S3AddressingStyle::VirtualHost;

    let builder = s3_config.into_builder();
    let op = Operator::new(builder)?.layer(RetryLayer::new()).finish();

    index(
        target_platform,
        op,
        repodata_patch,
        write_zst,
        write_shards,
        force,
        max_parallel,
        multi_progress,
    )
    .await
    .map(|_| ())
}

/// Create a new `repodata.json` for all packages in the given operator's root.
///
/// If `target_platform` is `Some`, only that specific subdir is indexed.
/// Otherwise, indexes all subdirs and creates a `repodata.json` for each.
///
/// The function takes roughly the following steps:
///
/// 1. Get all subdirs and create `noarch` and `target_platform` if they do not exist.
/// 2. Iterate subdirs and index each subdir:
///    1. Collect all uploaded packages in subdir
///    2. Collect all registered packages from `repodata.json` (if exists)
///    3. Determine which packages to add to and to delete from `repodata.json`
///    4. Write `repodata.json` back using conditional writes to prevent race conditions
///
/// Returns `IndexStats` containing statistics about the indexing operation,
/// including the number of packages added/removed and retry counts per subdir.
#[allow(clippy::too_many_arguments)]
pub async fn index(
    target_platform: Option<Platform>,
    op: Operator,
    repodata_patch: Option<String>,
    write_zst: bool,
    write_shards: bool,
    force: bool,
    max_parallel: usize,
    multi_progress: Option<MultiProgress>,
) -> anyhow::Result<IndexStats> {
    let entries = op.list_with("").await?;

    // If requested `target_platform` subdir does not exist, we create it.
    let mut subdirs = if let Some(target_platform) = target_platform {
        if !op.exists(&format!("{}/", target_platform.as_str())).await? {
            tracing::debug!("Did not find {target_platform} subdir, creating.");
            op.create_dir(&format!("{}/", target_platform.as_str()))
                .await?;
        }
        // Limit subdirs to only the requested `target_platform`.
        HashSet::from([target_platform])
    } else {
        entries
            .iter()
            .filter_map(|entry| {
                if entry.metadata().mode().is_dir() && entry.name() != "/" {
                    // Directory entries always end with `/`.
                    Some(entry.name().trim_end_matches('/').to_string())
                } else {
                    None
                }
            })
            .filter_map(|s| Platform::from_str(&s).ok())
            .collect::<HashSet<_>>()
    };

    if !op
        .exists(&format!("{}/", Platform::NoArch.as_str()))
        .await?
    {
        // If `noarch` subdir does not exist, we create it.
        tracing::debug!("Did not find noarch subdir, creating.");
        op.create_dir(&format!("{}/", Platform::NoArch.as_str()))
            .await?;
        subdirs.insert(Platform::NoArch);
    }

    let repodata_patch = if let Some(path) = repodata_patch {
        match ArchiveType::try_from(path.clone()) {
            Some(ArchiveType::Conda) => {}
            Some(ArchiveType::TarBz2) | None => {
                return Err(anyhow::anyhow!(
                    "Only .conda packages are supported for repodata patches. Got: {path}",
                ))
            }
        }
        let repodata_patch_path = format!("noarch/{path}");
        let repodata_patch_bytes = op.read(&repodata_patch_path).await?.to_bytes();
        let reader = Cursor::new(repodata_patch_bytes);
        let repodata_patch = repodata_patch_from_conda_package_stream(reader)?;
        Some(repodata_patch)
    } else {
        None
    };

    let semaphore = Semaphore::new(max_parallel);
    let semaphore = Arc::new(semaphore);

    let mut tasks: Vec<(Platform, _)> = Vec::new();
    for subdir in subdirs.iter() {
        // Create a separate cache for each subdir.
        // The cache persists across retry attempts for this specific subdir.
        let cache = cache::PackageRecordCache::new();

        let task = index_subdir(
            *subdir,
            op.clone(),
            force,
            write_zst,
            write_shards,
            repodata_patch
                .as_ref()
                .and_then(|p| p.subdirs.get(&subdir.to_string()).cloned()),
            multi_progress.clone(),
            semaphore.clone(),
            cache,
        )
        .instrument(tracing::info_span!("index_subdir", subdir = %subdir));
        tasks.push((*subdir, task));
    }

    let mut stats = IndexStats {
        subdirs: HashMap::new(),
    };

    for (subdir, task) in tasks {
        match task.await {
            Ok(subdir_stats) => {
                stats.subdirs.insert(subdir, subdir_stats);
            }
            Err(e) => {
                tracing::error!("Failed to process subdir: {e}");
                return Err(e);
            }
        }
    }
    Ok(stats)
}
