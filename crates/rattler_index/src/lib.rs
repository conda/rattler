//! Indexing of packages in a output folder to create up to date repodata.json
//! files
#![deny(missing_docs)]

pub mod cache;
/// Defines errors used in this crate.
pub mod error;
mod utils;

use crate::error::RepodataError;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{BufRead, BufReader, Cursor, Read, Seek},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::SystemTime,
};

use anyhow::{Context, Result};
use bytes::buf::Buf;
use fs_err::{self as fs};
use futures::{StreamExt, stream::FuturesUnordered};
use indexmap::IndexMap;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
#[cfg(feature = "s3")]
use opendal::layers::RetryLayer;
#[cfg(feature = "s3")]
use opendal::services::S3Config;
use opendal::{Configurator, Operator, services::FsConfig};
use rattler_conda_types::{
    ChannelInfo, ChannelNotice, ChannelNotices, ChannelRelations,
    MAX_REPODATA_REVISION_MESSAGE_BYTES, MatchSpec, PackageRecord, PackageRecordPatch,
    ParseMatchSpecOptions, PatchInstructions, Platform, RepoData, Shard, ShardedRepodata,
    ShardedSubdirInfo, UrlOrPath, V3Extensions, V3Packages, WhlPackageRecord,
    package::{
        CondaArchiveType, DistArchiveIdentifier, DistArchiveType, IndexJson, PackageFile,
        RunExportsJson, WheelArchiveType,
    },
};
pub use rattler_conda_types::{
    RepodataRevision, RepodataRevisionMetadata, RepodataRevisionSelection, RepodataRevisions,
};
pub use rattler_config::config::index::{
    IndexChannelConfig, IndexConfig, PackageRevisionAssignment,
};
use rattler_digest::Sha256Hash;
use rattler_package_streaming::{
    read,
    seek::{self, stream_conda_content},
};
#[cfg(feature = "s3")]
use rattler_s3::ResolvedS3Credentials;
use retry_policies::{Jitter, RetryDecision, RetryPolicy, policies::ExponentialBackoff};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tracing::Instrument;
#[cfg(feature = "s3")]
use url::Url;

/// Metadata published while indexing a channel.
///
/// Distinct from [`IndexChannelConfig`] — that type also describes indexer
/// behavior knobs (zst, shards, revisions, ...). This type contains metadata
/// written to generated repodata and the channel-root `notices.json` file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChannelMetadata {
    /// The `info.base_url` value written to `repodata.json`.
    pub base_url: Option<String>,
    /// The `info.channel_relations` value written to `repodata.json`.
    pub channel_relations: Option<ChannelRelations>,
    /// CEP-6 notices to write to the channel root.
    ///
    /// `None` leaves an existing `notices.json` untouched, while `Some` writes
    /// the supplied notices (including an explicitly empty list).
    pub notices: Option<Vec<ChannelNotice>>,
}

impl ChannelMetadata {
    /// Pull the metadata fields out of an [`IndexChannelConfig`].
    pub fn from_index_config(config: &IndexChannelConfig) -> Self {
        Self {
            base_url: config.base_url.clone(),
            channel_relations: config
                .channel_relations
                .clone()
                .filter(|relations| !relations.is_empty()),
            notices: config.notices.clone(),
        }
    }
}

/// Configuration for precondition checks during file operations.
///
/// Precondition checks use `ETags` and timestamps to detect concurrent modifications
/// and prevent race conditions when multiple processes are indexing simultaneously.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreconditionChecks {
    /// Enable precondition checks (default behavior).
    /// This provides protection against concurrent modifications.
    #[default]
    Enabled,
    /// Disable precondition checks.
    /// Use this when working with S3 implementations that don't fully support
    /// conditional requests, or when you're certain no concurrent indexing occurs.
    Disabled,
}

impl PreconditionChecks {
    /// Returns true if precondition checks are enabled
    pub fn is_enabled(self) -> bool {
        matches!(self, PreconditionChecks::Enabled)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct IndexedPackageRecord {
    record: PackageRecord,
    repodata_revision: RepodataRevision,
    wheel_url: Option<UrlOrPath>,
}

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
const CHANNEL_NOTICES: &str = "notices.json";
const ZSTD_REPODATA_COMPRESSION_LEVEL: i32 = 19;
const CACHE_CONTROL_IMMUTABLE: &str = "public, max-age=31536000, immutable";
const CACHE_CONTROL_REPODATA: &str = "public, max-age=300"; // 5 minutes

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
    indexed_package_record_from_index_json(package_as_bytes, index_json_reader)
        .map(|indexed| indexed.record)
}

/// Extract an indexed package record from an `index.json` file.
fn indexed_package_record_from_index_json<T: Read>(
    package_as_bytes: impl AsRef<[u8]>,
    index_json_reader: &mut T,
) -> std::io::Result<IndexedPackageRecord> {
    let index = IndexJson::from_reader(index_json_reader)?;
    index
        .validate()
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let repodata_revision = index.required_repodata_revision();

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
        extra_depends: index.extra_depends,
        constrains: index.constrains,
        track_features: index.track_features,
        features: index.features,
        flags: index.flags,
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

    Ok(IndexedPackageRecord {
        record: package_record,
        repodata_revision,
        wheel_url: None,
    })
}

fn repodata_patch_from_conda_package_stream<'a>(
    package: impl Read + Seek + 'a,
) -> anyhow::Result<rattler_conda_types::RepoDataPatch> {
    let mut subdirs = HashMap::default();

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

/// Extract the package record from a conda package archive.
///
/// This dispatches to the correct reader for `.conda` and `.tar.bz2` package
/// archives based on the file extension.
pub fn package_record_from_archive(file: &Path) -> std::io::Result<PackageRecord> {
    match CondaArchiveType::try_from(file).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unsupported package archive: {}", file.display()),
        )
    })? {
        CondaArchiveType::TarBz2 => package_record_from_tar_bz2(file),
        CondaArchiveType::Conda => package_record_from_conda(file),
    }
}

fn read_indexed_json_from_archive(
    bytes: &Vec<u8>,
    archive: &mut tar::Archive<impl Read>,
) -> std::io::Result<IndexedPackageRecord> {
    let mut index_json = None;
    let mut run_exports_json = None;
    for entry in archive.entries()?.flatten() {
        let mut entry = entry;
        let path = entry.path()?;
        if path.as_os_str().eq("info/index.json") {
            index_json = Some(indexed_package_record_from_index_json(bytes, &mut entry)?);
        } else if path.as_os_str().eq("info/run_exports.json") {
            run_exports_json = Some(RunExportsJson::from_reader(&mut entry)?);
        }
    }

    if let Some(mut index_json) = index_json {
        index_json.record.run_exports = run_exports_json;
        return Ok(index_json);
    }

    Err(std::io::Error::other("No index.json found"))
}

fn read_index_json_from_archive(
    bytes: &Vec<u8>,
    archive: &mut tar::Archive<impl Read>,
) -> std::io::Result<PackageRecord> {
    read_indexed_json_from_archive(bytes, archive).map(|indexed| indexed.record)
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

fn indexed_package_record_from_tar_bz2_reader(
    reader: impl BufRead,
) -> std::io::Result<IndexedPackageRecord> {
    let bytes = reader.bytes().collect::<Result<Vec<u8>, _>>()?;
    let reader = Cursor::new(&bytes);
    let mut archive = read::stream_tar_bz2(reader);
    for entry in archive.entries()?.flatten() {
        let mut entry = entry;
        let path = entry.path()?;
        if path.as_os_str().eq("info/index.json") {
            return indexed_package_record_from_index_json(&bytes, &mut entry);
        }
    }
    Err(std::io::Error::other("No index.json found"))
}

fn indexed_package_record_from_conda_reader(
    reader: impl BufRead,
) -> std::io::Result<IndexedPackageRecord> {
    let bytes = reader.bytes().collect::<Result<Vec<u8>, _>>()?;
    let reader = Cursor::new(&bytes);
    let mut archive = seek::stream_conda_info(reader).expect("Could not open conda file");
    read_indexed_json_from_archive(&bytes, &mut archive)
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
fn parse_package_buffer(
    buffer: opendal::Buffer,
    filename: &str,
) -> std::io::Result<IndexedPackageRecord> {
    let reader = buffer.reader();
    let archive_type = DistArchiveType::try_from(filename).unwrap();
    match archive_type {
        DistArchiveType::Conda(CondaArchiveType::TarBz2) => {
            indexed_package_record_from_tar_bz2_reader(reader)
        }
        DistArchiveType::Conda(CondaArchiveType::Conda) => {
            indexed_package_record_from_conda_reader(reader)
        }
        DistArchiveType::Wheel(WheelArchiveType::Whl) => Err(std::io::Error::other(
            "Package type \".whl\" not yet supported.",
        )),
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
/// Returns the parsed package record on success.
async fn read_and_parse_package(
    op: &Operator,
    cache: &cache::PackageRecordCache,
    subdir: Platform,
    filename: &str,
) -> std::io::Result<IndexedPackageRecord> {
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
                    file_existed: true, // File exists since we got its metadata from stat
                    precondition_checks: PreconditionChecks::Enabled, // Always enabled for cache reads
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
    pub last_modified: Option<opendal::raw::Timestamp>,
    /// Whether the file existed when metadata was collected
    pub file_existed: bool,
    /// The precondition checks configuration when this metadata was collected
    pub precondition_checks: PreconditionChecks,
}

impl RepodataFileMetadata {
    /// Collect metadata for a file without reading its contents.
    /// Returns metadata with None values if the file doesn't exist or if precondition checks are disabled.
    pub async fn new(
        op: &Operator,
        path: &str,
        precondition_checks: PreconditionChecks,
    ) -> opendal::Result<Self> {
        // If precondition checks are disabled, return empty metadata
        if !precondition_checks.is_enabled() {
            return Ok(Self {
                etag: None,
                last_modified: None,
                file_existed: false,
                precondition_checks,
            });
        }

        match op.stat(path).await {
            Ok(metadata) => Ok(Self {
                etag: metadata.etag().map(str::to_owned),
                last_modified: metadata.last_modified(),
                file_existed: true,
                precondition_checks,
            }),
            Err(e) if e.kind() == opendal::ErrorKind::NotFound => Ok(Self {
                etag: None,
                last_modified: None,
                file_existed: false,
                precondition_checks,
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
        precondition_checks: PreconditionChecks,
    ) -> opendal::Result<Self> {
        // Always track repodata.json
        let repodata =
            RepodataFileMetadata::new(op, &format!("{subdir}/{REPODATA}"), precondition_checks)
                .await?;

        // Track repodata_from_packages.json if patches are used
        let repodata_from_packages = if has_patch {
            Some(
                RepodataFileMetadata::new(
                    op,
                    &format!("{subdir}/{REPODATA_FROM_PACKAGES}"),
                    precondition_checks,
                )
                .await?,
            )
        } else {
            None
        };

        let repodata_zst = if write_zst {
            Some(
                RepodataFileMetadata::new(
                    op,
                    &format!("{subdir}/{REPODATA}.zst"),
                    precondition_checks,
                )
                .await?,
            )
        } else {
            None
        };

        let repodata_shards = if write_shards {
            Some(
                RepodataFileMetadata::new(
                    op,
                    &format!("{subdir}/{REPODATA_SHARDS}"),
                    precondition_checks,
                )
                .await?,
            )
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
    repodata_revisions: Vec<RepodataRevisionSelection>,
    package_revision_assignment: PackageRevisionAssignment,
    channel_metadata: ChannelMetadata,
    repodata_patch: Option<PatchInstructions>,
    progress: Option<MultiProgress>,
    semaphore: Arc<Semaphore>,
    cache: cache::PackageRecordCache,
    precondition_checks: PreconditionChecks,
) -> Result<SubdirIndexStats, RepodataError> {
    // Use write_retry_policy for handling lock contention during repodata writes
    // This will retry for 10 minutes with longer backoff durations (10s, 30s, 60s, etc.)
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
            repodata_revisions.clone(),
            package_revision_assignment,
            channel_metadata.clone(),
            repodata_patch.clone(),
            progress.clone(),
            semaphore.clone(),
            cache.clone(),
            precondition_checks,
        )
        .await
        {
            Ok(mut stats) => {
                stats.retries = current_try;
                return Ok(stats);
            }
            Err(e) => {
                // Check if this is a race condition error that we should retry
                let is_retryable_condition_error = match &e {
                    RepodataError::Opendal(opendal_err) => {
                        matches!(
                            opendal_err.kind(),
                            opendal::ErrorKind::ConditionNotMatch | opendal::ErrorKind::Unexpected
                        ) && {
                            // For Unexpected errors, check if it's the HTTP 409 ConditionalRequestConflict
                            let error_str = format!("{opendal_err:?}");
                            error_str.contains("ConditionalRequestConflict")
                                || error_str.contains("status: 409")
                                || opendal_err.kind() == opendal::ErrorKind::ConditionNotMatch
                        }
                    }
                    _ => false,
                };

                if is_retryable_condition_error {
                    // Race condition detected - should we retry?
                    match retry_policy.should_retry(request_start_time, current_try as u32) {
                        RetryDecision::Retry { execute_after } => {
                            let duration = execute_after
                                .duration_since(SystemTime::now())
                                .unwrap_or_default();

                            tracing::warn!(
                                "Detected concurrent modification of repodata for {} (attempt {}/max). \
                                 Error: {:?}. Retrying in {:?}.",
                                subdir,
                                current_try + 1,
                                e,
                                duration
                            );
                            tokio::time::sleep(duration).await;
                            current_try += 1;
                            continue;
                        }
                        RetryDecision::DoNotRetry => {
                            tracing::error!(
                                "Max retries exceeded for {subdir}. Final error: {e:?}"
                            );
                            return Err(e);
                        }
                    }
                }
                // Not a race condition error, propagate immediately
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
    repodata_revisions: Vec<RepodataRevisionSelection>,
    package_revision_assignment: PackageRevisionAssignment,
    channel_metadata: ChannelMetadata,
    repodata_patch: Option<PatchInstructions>,
    progress: Option<MultiProgress>,
    semaphore: Arc<Semaphore>,
    cache: cache::PackageRecordCache,
    precondition_checks: PreconditionChecks,
) -> Result<SubdirIndexStats, RepodataError> {
    // Step 1: Collect ETags/metadata for all critical files upfront
    let metadata = RepodataMetadataCollection::new(
        &op,
        subdir,
        repodata_patch.is_some(),
        write_zst,
        write_shards,
        precondition_checks,
    )
    .await?;

    // Step 2: Read previous typed records. In patch mode they come from the
    // unpatched source file, while published metadata and opaque v3 buckets
    // always come from repodata.json.
    let package_source = if repodata_patch.is_some() {
        read_existing_repodata(
            &op,
            &format!("{subdir}/{REPODATA_FROM_PACKAGES}"),
            metadata.repodata_from_packages.as_ref().unwrap(),
        )
        .await?
    } else {
        read_existing_repodata(&op, &format!("{subdir}/{REPODATA}"), &metadata.repodata).await?
    };
    let existing_repodata = if repodata_patch.is_some() {
        let published =
            read_existing_repodata(&op, &format!("{subdir}/{REPODATA}"), &metadata.repodata)
                .await?;
        merge_patch_repodata(package_source, published)
    } else {
        package_source
    };

    let ExistingRepodata {
        packages: mut registered_packages,
        v3_extensions,
        repodata_revisions: existing_repodata_revisions,
    } = if force {
        ExistingRepodata {
            packages: HashMap::default(),
            v3_extensions: existing_repodata.v3_extensions,
            repodata_revisions: existing_repodata.repodata_revisions,
        }
    } else {
        existing_repodata
    };

    // List all the packages in the subdirectory.
    let uploaded_packages: HashSet<DistArchiveIdentifier> = op
        .list_with(&format!("{}/", subdir.as_str()))
        .await?
        .iter()
        .filter_map(|entry| {
            if entry.metadata().mode().is_file() {
                let filename = entry.name().to_string();
                // Check if the file is an archive package file.
                DistArchiveIdentifier::try_from_filename(&filename)
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

    for filename in &packages_to_delete {
        registered_packages.remove(filename);
    }

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

                let record =
                    read_and_parse_package(&op, &cache, subdir, &filename.to_file_name()).await?;

                pb.inc(1);
                Ok::<(DistArchiveIdentifier, IndexedPackageRecord), std::io::Error>((
                    filename, record,
                ))
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
                return Err(RepodataError::Other(anyhow::anyhow!(e)));
            }
            Err(join_err) => {
                tasks.clear();
                tracing::error!("Task panicked: {}", join_err);
                pb.abandon_with_message(format!(
                    "{} {}",
                    console::style("Failed to index").red(),
                    console::style(subdir.as_str()).dim()
                ));
                return Err(join_err.into());
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

    let mut packages: IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState> =
        IndexMap::default();
    let mut conda_packages: IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState> =
        IndexMap::default();
    let mut v3 = V3Packages {
        extensions: v3_extensions,
        ..V3Packages::default()
    };
    let latest_revision = latest_repodata_revision(&repodata_revisions);
    for (filename, package) in registered_packages {
        let revision =
            package_revision_assignment.assign(package.repodata_revision, latest_revision);
        insert_package_record_by_revision(
            &mut packages,
            &mut conda_packages,
            &mut v3,
            filename,
            package,
            revision,
        )?;
    }

    // TODO: don't serialize run_exports and purls but in their own files
    let repodata_version = if channel_metadata.base_url.is_some() {
        2
    } else {
        1
    };
    let repodata_before_patches = RepoData {
        info: Some(ChannelInfo {
            subdir: Some(subdir.to_string()),
            base_url: channel_metadata.base_url,
            repodata_revisions: repodata_revisions_for_packages(
                &repodata_revisions,
                &existing_repodata_revisions,
                &packages,
                &conda_packages,
                &v3,
            ),
            channel_relations: channel_metadata.channel_relations,
        }),
        packages,
        conda_packages,
        v3,
        removed: HashSet::default(),
        version: Some(repodata_version),
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

fn serialize_msgpack_zst<T>(val: &T) -> Result<Vec<u8>, RepodataError>
where
    T: Serialize + ?Sized,
{
    let msgpack = rmp_serde::to_vec_named(val)?;
    let encoded = zstd::stream::encode_all(&msgpack[..], 0)?;
    Ok(encoded)
}

fn validate_configured_repodata_revisions(
    revisions: &[RepodataRevisionSelection],
) -> Result<(), RepodataError> {
    for revision in revisions {
        if revision.revision != RepodataRevision::V3 {
            return Err(RepodataError::Other(anyhow::anyhow!(
                "repodata revision {} cannot be configured; only v3 is selectable and the legacy layout is implicit",
                revision.revision
            )));
        }
        if revision
            .message
            .as_ref()
            .is_some_and(|message| message.len() > MAX_REPODATA_REVISION_MESSAGE_BYTES)
        {
            return Err(RepodataError::Other(anyhow::anyhow!(
                "repodata revision messages may not exceed {MAX_REPODATA_REVISION_MESSAGE_BYTES} bytes"
            )));
        }
    }
    Ok(())
}

fn latest_repodata_revision(revisions: &[RepodataRevisionSelection]) -> RepodataRevision {
    revisions
        .iter()
        .map(|revision| revision.revision)
        .max()
        .unwrap_or(RepodataRevision::Legacy)
}

#[derive(Default)]
struct ExistingRepodata {
    packages: ahash::HashMap<DistArchiveIdentifier, IndexedPackageRecord>,
    v3_extensions: V3Extensions,
    repodata_revisions: RepodataRevisions,
}

fn merge_patch_repodata(
    package_source: ExistingRepodata,
    published: ExistingRepodata,
) -> ExistingRepodata {
    ExistingRepodata {
        packages: package_source.packages,
        v3_extensions: published.v3_extensions,
        repodata_revisions: published.repodata_revisions,
    }
}

async fn read_existing_repodata(
    op: &Operator,
    repodata_path: &str,
    metadata: &RepodataFileMetadata,
) -> Result<ExistingRepodata, RepodataError> {
    match crate::utils::read_with_metadata_check(op, repodata_path, metadata).await {
        Ok(bytes) => {
            let bytes = bytes.to_vec();
            reject_unsupported_producer_revisions(&bytes)?;
            match serde_json::from_slice::<RepoData>(&bytes) {
                Ok(repodata) => Ok(package_records_from_repodata(repodata)),
                Err(err) => {
                    tracing::warn!(
                        "Failed to parse {repodata_path}: {err}. Not reusing content from this file"
                    );
                    Ok(ExistingRepodata::default())
                }
            }
        }
        Err(err) if err.kind() == opendal::ErrorKind::NotFound => {
            tracing::info!("Could not find {repodata_path}. Creating new one.");
            Ok(ExistingRepodata::default())
        }
        Err(err) => Err(err.into()),
    }
}

fn reject_unsupported_producer_revisions(bytes: &[u8]) -> Result<(), RepodataError> {
    let Ok(serde_json::Value::Object(repodata)) = serde_json::from_slice(bytes) else {
        return Ok(());
    };

    for key in repodata.keys() {
        let is_revision_key = key
            .strip_prefix('v')
            .or_else(|| key.strip_prefix('V'))
            .is_some_and(|number| {
                !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
            });
        if is_revision_key && key != "v3" {
            return Err(RepodataError::Other(anyhow::anyhow!(
                "repodata producer map {key} is not supported by this indexer"
            )));
        }
    }

    Ok(())
}

fn package_records_from_repodata(repodata: RepoData) -> ExistingRepodata {
    let repodata_revisions = repodata
        .info
        .as_ref()
        .map(|info| info.repodata_revisions.clone())
        .unwrap_or_default();
    let mut packages = ahash::HashMap::default();

    packages.extend(
        repodata
            .packages
            .into_iter()
            .chain(repodata.conda_packages)
            .map(|(identifier, record)| {
                (
                    identifier,
                    IndexedPackageRecord {
                        record,
                        repodata_revision: RepodataRevision::Legacy,
                        wheel_url: None,
                    },
                )
            }),
    );

    let (v3_records, v3_extensions) = repodata.v3.into_records_with_url_and_extensions();
    packages.extend(v3_records.map(|(identifier, record, wheel_url)| {
        (
            identifier,
            IndexedPackageRecord {
                record,
                repodata_revision: RepodataRevision::V3,
                wheel_url,
            },
        )
    }));

    ExistingRepodata {
        packages,
        v3_extensions,
        repodata_revisions,
    }
}

#[allow(clippy::too_many_arguments)]
fn insert_package_record_by_revision(
    packages: &mut IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState>,
    conda_packages: &mut IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState>,
    v3: &mut V3Packages,
    filename: DistArchiveIdentifier,
    package: IndexedPackageRecord,
    revision: RepodataRevision,
) -> Result<(), RepodataError> {
    let IndexedPackageRecord {
        record, wheel_url, ..
    } = package;

    if revision.uses_legacy_package_layout() {
        match filename.archive_type {
            DistArchiveType::Conda(CondaArchiveType::TarBz2) => {
                packages.insert(filename, record);
            }
            DistArchiveType::Conda(CondaArchiveType::Conda) => {
                conda_packages.insert(filename, record);
            }
            _ => {
                return Err(RepodataError::Other(anyhow::anyhow!(
                    "archive type '{:?}' is not supported in legacy repodata maps",
                    filename.archive_type
                )));
            }
        }
    } else if revision == RepodataRevision::V3 {
        match filename.archive_type {
            DistArchiveType::Conda(CondaArchiveType::TarBz2) => {
                v3.tar_bz2.insert(filename.identifier, record);
            }
            DistArchiveType::Conda(CondaArchiveType::Conda) => {
                v3.conda.insert(filename.identifier, record);
            }
            DistArchiveType::Wheel(WheelArchiveType::Whl) => {
                let url = wheel_url.ok_or_else(|| {
                    RepodataError::Other(anyhow::anyhow!(
                        "indexing new wheel packages into v3 repodata is not supported yet"
                    ))
                })?;
                v3.whl.insert(
                    filename.identifier,
                    WhlPackageRecord {
                        package_record: record,
                        url,
                    },
                );
            }
        }
    } else {
        return Err(RepodataError::Other(anyhow::anyhow!(
            "repodata revision {revision} is not supported by this indexer"
        )));
    }

    Ok(())
}

#[derive(Default)]
struct RevisionStats {
    n_packages: u64,
    oldest: Option<rattler_conda_types::utils::TimestampMs>,
    newest: Option<rattler_conda_types::utils::TimestampMs>,
}

impl RevisionStats {
    fn add(&mut self, record: &PackageRecord) {
        self.n_packages += 1;
        if let Some(timestamp) = record.timestamp_for_indexing() {
            self.oldest = Some(
                self.oldest
                    .map_or(timestamp, |oldest| oldest.min(timestamp)),
            );
            self.newest = Some(
                self.newest
                    .map_or(timestamp, |newest| newest.max(timestamp)),
            );
        }
    }
}

fn repodata_revisions_for_packages(
    configured: &[RepodataRevisionSelection],
    existing: &RepodataRevisions,
    legacy_packages: &IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState>,
    legacy_conda_packages: &IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState>,
    v3: &V3Packages,
) -> RepodataRevisions {
    // `BTreeMap` keeps the result ordered ascending regardless of input order.
    // Existing package statistics are deliberately discarded: generated
    // statistics always describe the typed records written below.
    let mut revisions = existing
        .iter()
        .filter(|(revision, _)| {
            revision.uses_legacy_package_layout() || **revision == RepodataRevision::V3
        })
        .map(|(revision, metadata)| {
            (
                *revision,
                RepodataRevisionMetadata {
                    message: metadata.message.clone(),
                    ..RepodataRevisionMetadata::default()
                },
            )
        })
        .collect::<BTreeMap<_, RepodataRevisionMetadata>>();
    for info in configured {
        if let Some(message) = &info.message {
            revisions.entry(info.revision).or_default().message = Some(message.clone());
        } else {
            revisions.entry(info.revision).or_default();
        }
    }

    let mut stats = BTreeMap::<RepodataRevision, RevisionStats>::new();
    for record in legacy_packages
        .values()
        .chain(legacy_conda_packages.values())
    {
        stats
            .entry(RepodataRevision::Legacy)
            .or_default()
            .add(record);
    }
    for (_, record) in v3.records() {
        stats.entry(RepodataRevision::V3).or_default().add(record);
    }

    for (revision, revision_stats) in stats {
        let metadata = revisions.entry(revision).or_default();
        metadata.n_packages = Some(revision_stats.n_packages);
        metadata.oldest = revision_stats.oldest;
        metadata.newest = revision_stats.newest;
    }

    // Keep configured revisions with zero packages so clients can still
    // surface channel capability information.
    for metadata in revisions.values_mut() {
        if metadata.n_packages.is_none() {
            metadata.n_packages = Some(0);
        }
    }

    revisions.into_iter().collect()
}

fn canonicalize_v3_match_spec(field: &str, spec: &str) -> Result<String, RepodataError> {
    let parsed = MatchSpec::from_str(
        spec,
        ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3),
    )
    .with_context(|| format!("failed to parse {field} MatchSpec '{spec}' for v3 repodata"))?;
    Ok(parsed.to_canonical_string().with_context(|| {
        format!("failed to canonicalize {field} MatchSpec '{spec}' for v3 repodata")
    })?)
}

fn canonicalize_v3_match_specs(field: &str, specs: &mut [String]) -> Result<(), RepodataError> {
    for spec in specs {
        *spec = canonicalize_v3_match_spec(field, spec)?;
    }
    Ok(())
}

fn canonicalize_v3_package_record(record: &mut PackageRecord) -> Result<(), RepodataError> {
    canonicalize_v3_match_specs("depends", &mut record.depends)?;
    canonicalize_v3_match_specs("constrains", &mut record.constrains)?;
    for (extra, specs) in &mut record.extra_depends {
        canonicalize_v3_match_specs(&format!("extra_depends.{extra}"), specs)?;
    }
    Ok(())
}

fn validate_v3_package_patch(patch: &PackageRecordPatch) -> Result<(), RepodataError> {
    if let Some(specs) = &patch.depends {
        for spec in specs {
            canonicalize_v3_match_spec("depends", spec)?;
        }
    }
    if let Some(specs) = &patch.constrains {
        for spec in specs {
            canonicalize_v3_match_spec("constrains", spec)?;
        }
    }
    if let Some(extra_depends) = &patch.extra_depends {
        for (extra, specs) in extra_depends {
            for spec in specs {
                canonicalize_v3_match_spec(&format!("extra_depends.{extra}"), spec)?;
            }
        }
    }
    Ok(())
}

fn validate_indexer_patch(instructions: &PatchInstructions) -> Result<(), RepodataError> {
    let legacy_extra_depends = instructions
        .packages
        .values()
        .chain(instructions.conda_packages.values())
        .any(|patch| {
            patch
                .extra_depends
                .as_ref()
                .is_some_and(|extra_depends| !extra_depends.is_empty())
        });
    if legacy_extra_depends {
        return Err(RepodataError::Patch(
            "legacy repodata patches cannot set extra_depends; use a v3 patch bucket".to_string(),
        ));
    }

    for patch in instructions
        .v3
        .tar_bz2
        .values()
        .chain(instructions.v3.conda.values())
        .chain(instructions.v3.whl.values())
    {
        validate_v3_package_patch(patch)?;
    }
    Ok(())
}

/// Canonicalizes only indexer-produced v3 package metadata after all source and
/// patch inputs have been applied. General repodata serialization remains
/// permissive so consumers can round-trip legacy or third-party v3 data.
fn canonicalize_indexer_v3(repodata: &mut RepoData) -> Result<(), RepodataError> {
    for record in repodata.v3.tar_bz2.values_mut() {
        canonicalize_v3_package_record(record)?;
    }
    for record in repodata.v3.conda.values_mut() {
        canonicalize_v3_package_record(record)?;
    }
    for record in repodata.v3.whl.values_mut() {
        canonicalize_v3_package_record(&mut record.package_record)?;
    }
    Ok(())
}

/// Serialize repodata emitted by this indexer.
///
/// Producer output always advertises all supported package layouts, including
/// an empty `v3` map. `RepoData` itself intentionally remains permissive when
/// round-tripping older producer output that omitted this map.
fn serialize_indexer_repodata(repodata: &RepoData) -> Result<Vec<u8>, RepodataError> {
    if repodata.v3.is_empty() {
        #[derive(Serialize)]
        struct WithEmptyV3<'a> {
            #[serde(flatten)]
            repodata: &'a RepoData,
            v3: &'a V3Packages,
        }

        Ok(serde_json::to_vec(&WithEmptyV3 {
            repodata,
            v3: &repodata.v3,
        })?)
    } else {
        Ok(serde_json::to_vec(repodata)?)
    }
}

/// Write a `repodata.json` for all packages in the given configurator's root.
/// Uses conditional writes based on the provided metadata to prevent concurrent
/// modification issues.
pub async fn write_repodata(
    mut repodata: RepoData,
    repodata_patch: Option<PatchInstructions>,
    subdir: Platform,
    op: Operator,
    metadata: &RepodataMetadataCollection,
) -> Result<(), RepodataError> {
    if let Some(instructions) = repodata_patch.as_ref() {
        validate_indexer_patch(instructions)?;
    }
    canonicalize_indexer_v3(&mut repodata)?;
    let patched_repodata = if let Some(instructions) = repodata_patch {
        tracing::info!("Patching repodata");
        let mut patched_repodata = repodata.clone();
        patched_repodata.apply_patches(&instructions);
        canonicalize_indexer_v3(&mut patched_repodata)?;
        Some(patched_repodata)
    } else {
        None
    };

    // Finish all fallible transformation and canonicalization before publishing
    // either artifact, so invalid patch output cannot leave a partial update.
    if let Some(repodata_from_packages_metadata) = &metadata.repodata_from_packages {
        let unpatched_repodata_path = format!("{subdir}/{REPODATA_FROM_PACKAGES}");
        tracing::info!("Writing unpatched repodata to {unpatched_repodata_path}");
        let unpatched_repodata_bytes = serialize_indexer_repodata(&repodata)?;
        crate::utils::write_with_metadata_check(
            &op,
            &unpatched_repodata_path,
            unpatched_repodata_bytes,
            repodata_from_packages_metadata,
            Some(CACHE_CONTROL_REPODATA),
        )
        .await?;
    }

    let repodata = patched_repodata.unwrap_or(repodata);
    let repodata_bytes = serialize_indexer_repodata(&repodata)?;

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
            Some(CACHE_CONTROL_REPODATA),
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
        Some(CACHE_CONTROL_REPODATA),
    )
    .await?;

    if metadata.repodata_shards.is_some() {
        // See CEP 16 <https://github.com/conda/ceps/blob/main/cep-0016.md>
        tracing::info!("Creating sharded repodata");
        let mut shards_by_package_names: HashMap<String, Shard> = HashMap::new();
        let sharded_base_url = repodata
            .info
            .as_ref()
            .and_then(|info| info.base_url.clone())
            .unwrap_or_default();
        let sharded_repodata_revisions = repodata
            .info
            .as_ref()
            .map(|info| info.repodata_revisions.clone())
            .unwrap_or_default();
        let sharded_channel_relations = repodata
            .info
            .as_ref()
            .and_then(|info| info.channel_relations.clone());
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
        for (k, package_record) in repodata.v3.conda {
            let package_name = package_record.name.as_normalized();
            let shard = shards_by_package_names
                .entry(package_name.into())
                .or_default();
            shard.v3.conda.insert(k, package_record);
        }
        for (k, package_record) in repodata.v3.tar_bz2 {
            let package_name = package_record.name.as_normalized();
            let shard = shards_by_package_names
                .entry(package_name.into())
                .or_default();
            shard.v3.tar_bz2.insert(k, package_record);
        }
        for (k, package_record) in repodata.v3.whl {
            let package_name = package_record.package_record.name.as_normalized();
            let shard = shards_by_package_names
                .entry(package_name.into())
                .or_default();
            shard.v3.whl.insert(k, package_record);
        }
        for package in repodata.removed {
            let package_name = package.identifier.name.clone();
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
            .collect::<Result<HashMap<_, _>, RepodataError>>()?;

        let sharded_repodata = ShardedRepodata {
            info: ShardedSubdirInfo {
                subdir: subdir.to_string(),
                base_url: sharded_base_url,
                shards_base_url: "./shards/".into(),
                created_at: Some(jiff::Timestamp::now()),
                repodata_revisions: sharded_repodata_revisions,
                channel_relations: sharded_channel_relations,
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
                let shard_path = format!("{subdir}/shards/{}.msgpack.zst", hex::encode(digest));
                tracing::trace!("Writing repodata shard to {shard_path}");
                match op
                    .write_with(&shard_path, encoded_shard)
                    .if_not_exists(true)
                    .cache_control(CACHE_CONTROL_IMMUTABLE)
                    .await
                {
                    Err(e) if e.kind() == opendal::ErrorKind::ConditionNotMatch => {
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
                Some(CACHE_CONTROL_REPODATA),
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
    /// Repodata revisions to advertise in generated repodata.
    pub repodata_revisions: Vec<RepodataRevisionSelection>,
    /// How packages are assigned to repodata revisions.
    pub package_revision_assignment: PackageRevisionAssignment,
    /// Whether to force the index to be written.
    pub force: bool,
    /// The maximum number of parallel tasks to run.
    pub max_parallel: usize,
    /// The multi-progress bar to use for the index.
    pub multi_progress: Option<MultiProgress>,
}

/// Create a new `repodata.json` for all packages in the channel at the given
/// directory.
pub async fn index_fs(config: IndexFsConfig) -> anyhow::Result<()> {
    index_fs_with_channel_metadata(config, ChannelMetadata::default()).await
}

/// Create a new `repodata.json` for all packages in the channel at the given
/// directory and write channel metadata into the generated repodata.
pub async fn index_fs_with_channel_metadata(
    IndexFsConfig {
        channel,
        target_platform,
        repodata_patch,
        write_zst,
        write_shards,
        repodata_revisions,
        package_revision_assignment,
        force,
        max_parallel,
        multi_progress,
    }: IndexFsConfig,
    channel_metadata: ChannelMetadata,
) -> anyhow::Result<()> {
    let mut config = FsConfig::default();
    let root = channel.canonicalize()?;
    config.root = Some(root.to_string_lossy().to_string());
    // Write through a temp dir on the same volume and rename over the target,
    // so a memory-mapped repodata.json isn't truncated in place (fails with
    // ERROR_USER_MAPPED_FILE on Windows). `.tmp` is skipped during subdir
    // enumeration since it doesn't parse as a `Platform`.
    config.atomic_write_dir = Some(root.join(".tmp").to_string_lossy().to_string());
    let builder = config.into_builder();
    let op = Operator::new(builder)?.finish();
    index_with_channel_metadata(
        target_platform,
        op,
        repodata_patch,
        write_zst,
        write_shards,
        repodata_revisions,
        package_revision_assignment,
        force,
        max_parallel,
        multi_progress,
        PreconditionChecks::Disabled,
        channel_metadata,
    )
    .await
    .map(|_| ())
}

/// Configuration for `index_s3`
#[cfg(feature = "s3")]
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
    /// Repodata revisions to advertise in generated repodata.
    pub repodata_revisions: Vec<RepodataRevisionSelection>,
    /// How packages are assigned to repodata revisions.
    pub package_revision_assignment: PackageRevisionAssignment,
    /// Whether to force the index to be written.
    pub force: bool,
    /// The maximum number of parallel tasks to run.
    pub max_parallel: usize,
    /// The multi-progress bar to use for the index.
    pub multi_progress: Option<MultiProgress>,
    /// Configuration for precondition checks during file operations.
    pub precondition_checks: PreconditionChecks,
}

#[cfg(feature = "s3")]
fn s3_config(
    credentials: &ResolvedS3Credentials,
    channel: &Url,
) -> Result<S3Config, anyhow::Error> {
    let mut s3_config = S3Config::default();
    s3_config.root = Some(channel.path().to_string());
    s3_config.bucket = channel
        .host_str()
        .ok_or(anyhow::anyhow!("No bucket in S3 URL"))?
        .to_string();
    s3_config.region = Some(credentials.region.clone());
    s3_config.endpoint = Some(credentials.endpoint_url.to_string());
    s3_config.secret_access_key = Some(credentials.secret_access_key.clone());
    s3_config.access_key_id = Some(credentials.access_key_id.clone());
    s3_config.session_token = credentials.session_token.clone();
    s3_config.enable_virtual_host_style =
        credentials.addressing_style == rattler_s3::S3AddressingStyle::VirtualHost;

    Ok(s3_config)
}

/// Create a new `repodata.json` for all packages in the channel at the given S3
/// URL.
#[cfg(feature = "s3")]
pub async fn index_s3(config: IndexS3Config) -> anyhow::Result<()> {
    index_s3_with_channel_metadata(config, ChannelMetadata::default()).await
}

/// Create a new `repodata.json` for all packages in the channel at the given S3
/// URL and write channel metadata into the generated repodata.
#[cfg(feature = "s3")]
pub async fn index_s3_with_channel_metadata(
    IndexS3Config {
        channel,
        credentials,
        target_platform,
        repodata_patch,
        write_zst,
        write_shards,
        repodata_revisions,
        package_revision_assignment,
        force,
        max_parallel,
        multi_progress,
        precondition_checks,
    }: IndexS3Config,
    channel_metadata: ChannelMetadata,
) -> anyhow::Result<()> {
    // Create the S3 configuration for opendal.
    let s3_config = s3_config(&credentials, &channel)?;
    let builder = s3_config.into_builder();
    let op = Operator::new(builder)?.layer(RetryLayer::new()).finish();

    index_with_channel_metadata(
        target_platform,
        op,
        repodata_patch,
        write_zst,
        write_shards,
        repodata_revisions,
        package_revision_assignment,
        force,
        max_parallel,
        multi_progress,
        precondition_checks,
        channel_metadata,
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
    repodata_revisions: Vec<RepodataRevisionSelection>,
    package_revision_assignment: PackageRevisionAssignment,
    force: bool,
    max_parallel: usize,
    multi_progress: Option<MultiProgress>,
    precondition_checks: PreconditionChecks,
) -> anyhow::Result<IndexStats> {
    index_with_channel_metadata(
        target_platform,
        op,
        repodata_patch,
        write_zst,
        write_shards,
        repodata_revisions,
        package_revision_assignment,
        force,
        max_parallel,
        multi_progress,
        precondition_checks,
        ChannelMetadata::default(),
    )
    .await
}

/// Create a new `repodata.json` for all packages in the given operator's root
/// and write channel metadata into the generated repodata.
#[allow(clippy::too_many_arguments)]
pub async fn index_with_channel_metadata(
    target_platform: Option<Platform>,
    op: Operator,
    repodata_patch: Option<String>,
    write_zst: bool,
    write_shards: bool,
    repodata_revisions: Vec<RepodataRevisionSelection>,
    package_revision_assignment: PackageRevisionAssignment,
    force: bool,
    max_parallel: usize,
    multi_progress: Option<MultiProgress>,
    precondition_checks: PreconditionChecks,
    channel_metadata: ChannelMetadata,
) -> anyhow::Result<IndexStats> {
    validate_configured_repodata_revisions(&repodata_revisions)?;

    let notices_metadata = if channel_metadata.notices.is_some() {
        Some(RepodataFileMetadata::new(&op, CHANNEL_NOTICES, precondition_checks).await?)
    } else {
        None
    };
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
        match DistArchiveType::try_from(path.clone()) {
            Some(DistArchiveType::Conda(CondaArchiveType::Conda)) => {}
            Some(
                DistArchiveType::Conda(CondaArchiveType::TarBz2)
                | DistArchiveType::Wheel(WheelArchiveType::Whl),
            )
            | None => {
                return Err(anyhow::anyhow!(
                    "Only .conda packages are supported for repodata patches. Got: {path}",
                ));
            }
        }
        let repodata_patch_path = format!("noarch/{path}");
        let repodata_patch_bytes = op.read(&repodata_patch_path).await?.to_bytes();
        let reader = Cursor::new(repodata_patch_bytes);
        let repodata_patch = repodata_patch_from_conda_package_stream(reader)?;
        for (subdir, instructions) in &repodata_patch.subdirs {
            validate_indexer_patch(instructions).map_err(|error| {
                anyhow::anyhow!("invalid repodata patch for subdir {subdir}: {error}")
            })?;
        }
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
            repodata_revisions.clone(),
            package_revision_assignment,
            channel_metadata.clone(),
            repodata_patch
                .as_ref()
                .and_then(|p| p.subdirs.get(&subdir.to_string()).cloned()),
            multi_progress.clone(),
            semaphore.clone(),
            cache,
            precondition_checks,
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
                return Err(e.into());
            }
        }
    }

    // Publish notices only after all repodata updates succeeded, so a failed
    // indexing operation cannot partially update channel-level messaging.
    if let (Some(notices), Some(metadata)) = (&channel_metadata.notices, notices_metadata.as_ref())
    {
        write_channel_notices_with_metadata(&op, notices, metadata).await?;
    }

    Ok(stats)
}

/// Write CEP-6 channel notices to the channel root.
pub async fn write_channel_notices(op: &Operator, notices: &[ChannelNotice]) -> anyhow::Result<()> {
    let metadata =
        RepodataFileMetadata::new(op, CHANNEL_NOTICES, PreconditionChecks::Disabled).await?;
    write_channel_notices_with_metadata(op, notices, &metadata).await
}

async fn write_channel_notices_with_metadata(
    op: &Operator,
    notices: &[ChannelNotice],
    metadata: &RepodataFileMetadata,
) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(&ChannelNotices {
        notices: notices.to_vec(),
    })?;
    let mut writer = op
        .write_with(CHANNEL_NOTICES, bytes)
        .content_type("application/json")
        .cache_control(CACHE_CONTROL_REPODATA);
    if metadata.precondition_checks.is_enabled() {
        if let Some(etag) = &metadata.etag {
            writer = writer.if_match(etag);
        } else if !metadata.file_existed {
            writer = writer.if_not_exists(true);
        }
    }
    writer.await?;
    Ok(())
}

/// Ensures that a channel has a valid `noarch/repodata.json` file.
///
/// If `noarch/repodata.json` doesn't exist, creates an empty one.
/// This is useful when publishing to a new channel to ensure it's
/// immediately usable.
pub async fn ensure_channel_initialized(op: &Operator) -> anyhow::Result<()> {
    ensure_channel_initialized_with_channel_metadata(op, ChannelMetadata::default()).await
}

/// Ensures that a channel has a valid `noarch/repodata.json` file and writes
/// channel metadata into the generated file if initialization is needed.
pub async fn ensure_channel_initialized_with_channel_metadata(
    op: &Operator,
    channel_metadata: ChannelMetadata,
) -> anyhow::Result<()> {
    let noarch_repodata_path = format!("{}/{REPODATA}", Platform::NoArch.as_str());

    if op.exists(&noarch_repodata_path).await? {
        tracing::debug!("Channel already initialized");
        return Ok(());
    }

    tracing::info!("Initializing channel with empty noarch/repodata.json");

    let noarch_path = format!("{}/", Platform::NoArch.as_str());
    if !op.exists(&noarch_path).await? {
        op.create_dir(&noarch_path).await?;
    }

    let repodata_version = if channel_metadata.base_url.is_some() {
        2
    } else {
        1
    };
    let empty_repodata = RepoData {
        info: Some(ChannelInfo {
            subdir: Some(Platform::NoArch.to_string()),
            base_url: channel_metadata.base_url,
            repodata_revisions: RepodataRevisions::new(),
            channel_relations: channel_metadata.channel_relations,
        }),
        packages: IndexMap::default(),
        conda_packages: IndexMap::default(),
        v3: V3Packages::default(),
        removed: HashSet::default(),
        version: Some(repodata_version),
    };

    let repodata_bytes = serialize_indexer_repodata(&empty_repodata)?;
    match op
        .write_with(&noarch_repodata_path, repodata_bytes)
        .if_not_exists(true)
        .cache_control(CACHE_CONTROL_REPODATA)
        .await
    {
        Ok(_) => {
            tracing::info!("Successfully initialized channel");
            Ok(())
        }
        Err(e) if e.kind() == opendal::ErrorKind::ConditionNotMatch => {
            // Another process created the file - that's fine, channel is initialized
            tracing::debug!("Channel already initialized by another process");
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// Ensures that a filesystem channel has a valid `noarch/repodata.json` file.
///
/// See [`ensure_channel_initialized`] for details.
pub async fn ensure_channel_initialized_fs(channel: &Path) -> anyhow::Result<()> {
    ensure_channel_initialized_fs_with_channel_metadata(channel, ChannelMetadata::default()).await
}

/// Ensures that a filesystem channel has a valid `noarch/repodata.json` file
/// and writes channel metadata into the generated file if initialization is
/// needed.
pub async fn ensure_channel_initialized_fs_with_channel_metadata(
    channel: &Path,
    channel_metadata: ChannelMetadata,
) -> anyhow::Result<()> {
    let mut config = FsConfig::default();
    let root = channel.canonicalize()?;
    config.root = Some(root.to_string_lossy().to_string());
    // Atomic writes, see `index_fs_with_channel_metadata`.
    config.atomic_write_dir = Some(root.join(".tmp").to_string_lossy().to_string());
    let op = Operator::new(config.into_builder())?.finish();
    ensure_channel_initialized_with_channel_metadata(&op, channel_metadata).await
}

/// Ensures that an S3 channel has a valid `noarch/repodata.json` file.
///
/// See [`ensure_channel_initialized`] for details.
#[cfg(feature = "s3")]
pub async fn ensure_channel_initialized_s3(
    channel: &Url,
    credentials: &ResolvedS3Credentials,
) -> anyhow::Result<()> {
    ensure_channel_initialized_s3_with_channel_metadata(
        channel,
        credentials,
        ChannelMetadata::default(),
    )
    .await
}

/// Ensures that an S3 channel has a valid `noarch/repodata.json` file and
/// writes channel metadata into the generated file if initialization is needed.
#[cfg(feature = "s3")]
pub async fn ensure_channel_initialized_s3_with_channel_metadata(
    channel: &Url,
    credentials: &ResolvedS3Credentials,
    channel_metadata: ChannelMetadata,
) -> anyhow::Result<()> {
    let s3_config = s3_config(credentials, channel)?;

    let op = Operator::new(s3_config.into_builder())?
        .layer(RetryLayer::new())
        .finish();
    ensure_channel_initialized_with_channel_metadata(&op, channel_metadata).await
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, str::FromStr};

    use indexmap::IndexMap;
    use rattler_conda_types::Version;
    use rattler_conda_types::{
        PackageName, UrlOrPath, WhlPackageRecord, package::ArchiveIdentifier,
    };

    use super::*;

    #[test]
    fn package_records_from_repodata_preserves_v3_wheels() {
        let identifier = ArchiveIdentifier::from_str("demo-1.0-py_0").unwrap();
        let package_record = PackageRecord::new(
            PackageName::new_unchecked("demo"),
            Version::from_str("1.0").unwrap(),
            "py_0".to_string(),
        );
        let wheel_url = UrlOrPath::Path("demo-1.0-py_0.whl".to_string());

        let mut repodata = RepoData {
            info: None,
            packages: IndexMap::default(),
            conda_packages: IndexMap::default(),
            v3: V3Packages::default(),
            removed: HashSet::default(),
            version: None,
        };
        repodata.v3.whl.insert(
            identifier.clone(),
            WhlPackageRecord {
                package_record,
                url: wheel_url.clone(),
            },
        );

        let ExistingRepodata {
            packages: records, ..
        } = package_records_from_repodata(repodata);
        let dist_identifier = DistArchiveIdentifier::new(identifier.clone(), WheelArchiveType::Whl);
        let indexed_record = records
            .get(&dist_identifier)
            .expect("v3 wheel should be preserved");
        assert_eq!(indexed_record.repodata_revision, RepodataRevision::V3);
        assert_eq!(indexed_record.wheel_url, Some(wheel_url));

        let (_, indexed_record) = records
            .into_iter()
            .next()
            .expect("v3 wheel should be present");
        let mut packages = IndexMap::default();
        let mut conda_packages = IndexMap::default();
        let mut v3 = V3Packages::default();
        insert_package_record_by_revision(
            &mut packages,
            &mut conda_packages,
            &mut v3,
            dist_identifier,
            indexed_record,
            RepodataRevision::V3,
        )
        .unwrap();

        assert!(packages.is_empty());
        assert!(conda_packages.is_empty());
        assert!(v3.whl.contains_key(&identifier));
    }

    #[test]
    fn legacy_revision_metadata_includes_configured_message_and_package_stats() {
        let mut legacy_packages = IndexMap::default();
        let mut legacy_conda_packages = IndexMap::default();
        let oldest: rattler_conda_types::utils::TimestampMs =
            serde_json::from_str("1710000000000").unwrap();
        let newest: rattler_conda_types::utils::TimestampMs =
            serde_json::from_str("1720000000000").unwrap();

        let mut tar_bz2_record = PackageRecord::new(
            PackageName::new_unchecked("legacy-tar"),
            Version::from_str("1.0").unwrap(),
            "0".to_string(),
        );
        tar_bz2_record.timestamp = Some(oldest);
        legacy_packages.insert(
            DistArchiveIdentifier::try_from_filename("legacy-tar-1.0-0.tar.bz2").unwrap(),
            tar_bz2_record,
        );

        let mut conda_record = PackageRecord::new(
            PackageName::new_unchecked("legacy-conda"),
            Version::from_str("1.0").unwrap(),
            "0".to_string(),
        );
        conda_record.timestamp = Some(newest);
        legacy_conda_packages.insert(
            DistArchiveIdentifier::try_from_filename("legacy-conda-1.0-0.conda").unwrap(),
            conda_record,
        );

        let existing = RepodataRevisions::from([(
            RepodataRevision::Legacy,
            RepodataRevisionMetadata {
                message: Some("stale message".to_string()),
                n_packages: Some(99),
                oldest: Some(newest),
                newest: Some(newest),
            },
        )]);
        let revisions = repodata_revisions_for_packages(
            &[],
            &existing,
            &legacy_packages,
            &legacy_conda_packages,
            &V3Packages::default(),
        );

        assert_eq!(revisions.len(), 1);
        let metadata = &revisions[&RepodataRevision::Legacy];
        assert_eq!(metadata.message.as_deref(), Some("stale message"));
        assert_eq!(metadata.n_packages, Some(2));
        assert_eq!(metadata.oldest, Some(oldest));
        assert_eq!(metadata.newest, Some(newest));
    }

    #[test]
    fn patch_reindex_uses_published_extensions_and_revision_messages() {
        let mut extensions = V3Extensions::default();
        extensions
            .insert("zip", serde_json::json!({ "future": true }))
            .unwrap();
        let published = ExistingRepodata {
            v3_extensions: extensions,
            repodata_revisions: RepodataRevisions::from([(
                RepodataRevision::V3,
                RepodataRevisionMetadata {
                    message: Some("published message".to_string()),
                    ..RepodataRevisionMetadata::default()
                },
            )]),
            ..ExistingRepodata::default()
        };

        let merged = merge_patch_repodata(ExistingRepodata::default(), published);
        assert_eq!(
            merged.v3_extensions.get("zip"),
            Some(&serde_json::json!({ "future": true }))
        );
        assert_eq!(
            merged.repodata_revisions[&RepodataRevision::V3]
                .message
                .as_deref(),
            Some("published message")
        );
    }

    #[test]
    fn indexer_rejects_unsupported_configured_revisions() {
        let err = validate_configured_repodata_revisions(&[RepodataRevisionSelection {
            revision: RepodataRevision::from(4),
            message: None,
        }])
        .unwrap_err();
        assert!(
            err.to_string().contains(
                "repodata revision v4 cannot be configured; only v3 is selectable and the legacy layout is implicit"
            )
        );
    }

    #[test]
    fn indexer_rejects_configured_legacy_revision() {
        let err = validate_configured_repodata_revisions(&[RepodataRevisionSelection {
            revision: RepodataRevision::Legacy,
            message: None,
        }])
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("only v3 is selectable and the legacy layout is implicit")
        );
    }

    #[test]
    fn indexer_rejects_oversized_revision_messages() {
        let err = validate_configured_repodata_revisions(&[RepodataRevisionSelection {
            revision: RepodataRevision::V3,
            message: Some("é".repeat(4097)),
        }])
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("repodata revision messages may not exceed 8192 bytes")
        );
    }

    #[test]
    fn indexer_rejects_unsupported_producer_maps() {
        for key in ["v4", "V3", "v03", "v18446744073709551616"] {
            let repodata =
                format!(r#"{{"packages": {{}}, "packages.conda": {{}}, "{key}": {{}}}}"#);
            let err = reject_unsupported_producer_revisions(repodata.as_bytes()).unwrap_err();
            assert!(
                err.to_string()
                    .contains(&format!("repodata producer map {key} is not supported")),
                "unexpected error for {key}: {err}"
            );
        }

        reject_unsupported_producer_revisions(
            br#"{"packages": {}, "packages.conda": {}, "v3": {}}"#,
        )
        .unwrap();
    }

    #[test]
    fn existing_revision_message_is_preserved_without_an_override() {
        let existing = RepodataRevisions::from([(
            RepodataRevision::V3,
            RepodataRevisionMetadata {
                message: Some("existing message".to_string()),
                n_packages: Some(99),
                ..RepodataRevisionMetadata::default()
            },
        )]);
        let empty = IndexMap::default();

        let preserved =
            repodata_revisions_for_packages(&[], &existing, &empty, &empty, &V3Packages::default());
        assert_eq!(
            preserved[&RepodataRevision::V3].message.as_deref(),
            Some("existing message")
        );
        assert_eq!(preserved[&RepodataRevision::V3].n_packages, Some(0));

        let overridden = repodata_revisions_for_packages(
            &[RepodataRevisionSelection {
                revision: RepodataRevision::V3,
                message: Some("caller message".to_string()),
            }],
            &existing,
            &empty,
            &empty,
            &V3Packages::default(),
        );
        assert_eq!(
            overridden[&RepodataRevision::V3].message.as_deref(),
            Some("caller message")
        );
    }

    #[tokio::test]
    async fn indexer_canonicalizes_original_and_patched_v3_dependencies() {
        let channel = tempfile::tempdir().unwrap();
        let mut config = FsConfig::default();
        config.root = Some(channel.path().to_string_lossy().to_string());
        let op = Operator::new(config.into_builder()).unwrap().finish();
        op.create_dir("noarch/").await.unwrap();

        let identifier = ArchiveIdentifier::from_str("v3-demo-1.0-0").unwrap();
        let mut v3_record = PackageRecord::new(
            PackageName::new_unchecked("v3-demo"),
            Version::from_str("1.0").unwrap(),
            "0".to_string(),
        );
        v3_record.depends = vec!["python >=3.10".to_string()];
        v3_record.constrains = vec!["python <3.13".to_string()];
        v3_record.extra_depends =
            BTreeMap::from([("test".to_string(), vec!["pytest >=8".to_string()])]);

        let legacy_filename = "legacy-1.0-0.tar.bz2";
        let mut legacy_record = PackageRecord::new(
            PackageName::new_unchecked("legacy"),
            Version::from_str("1.0").unwrap(),
            "0".to_string(),
        );
        legacy_record.depends = vec!["python >=3.10".to_string()];
        legacy_record.constrains = vec!["python <3.13".to_string()];
        legacy_record.extra_depends =
            BTreeMap::from([("test".to_string(), vec!["pytest >=8".to_string()])]);

        let mut repodata = RepoData {
            info: None,
            packages: IndexMap::default(),
            conda_packages: IndexMap::default(),
            v3: V3Packages::default(),
            removed: HashSet::default(),
            version: Some(1),
        };
        repodata.packages.insert(
            DistArchiveIdentifier::try_from_filename(legacy_filename).unwrap(),
            legacy_record,
        );
        repodata.v3.conda.insert(identifier.clone(), v3_record);

        let mut patched_conda = serde_json::Map::new();
        patched_conda.insert(
            identifier.to_string(),
            serde_json::json!({
                "depends": ["python >=3.11"],
                "constrains": ["python <3.14"],
                "extra_depends": { "test": ["pytest >=9"] }
            }),
        );
        let patched = serde_json::from_value::<PatchInstructions>(serde_json::json!({
            "v3": { "conda": patched_conda }
        }))
        .unwrap();
        let metadata = RepodataMetadataCollection::new(
            &op,
            Platform::NoArch,
            true,
            false,
            false,
            PreconditionChecks::Disabled,
        )
        .await
        .unwrap();
        write_repodata(repodata, Some(patched), Platform::NoArch, op, &metadata)
            .await
            .unwrap();

        let source: serde_json::Value = serde_json::from_slice(
            &std::fs::read(channel.path().join("noarch/repodata_from_packages.json")).unwrap(),
        )
        .unwrap();
        let published: serde_json::Value = serde_json::from_slice(
            &std::fs::read(channel.path().join("noarch/repodata.json")).unwrap(),
        )
        .unwrap();
        let identifier = identifier.to_string();

        for (repodata, expected) in [
            (
                &source,
                serde_json::json!({
                    "depends": ["python[version=\">=3.10\"]"],
                    "constrains": ["python[version=\"<3.13\"]"],
                    "extra_depends": { "test": ["pytest[version=\">=8\"]"] }
                }),
            ),
            (
                &published,
                serde_json::json!({
                    "depends": ["python[version=\">=3.11\"]"],
                    "constrains": ["python[version=\"<3.14\"]"],
                    "extra_depends": { "test": ["pytest[version=\">=9\"]"] }
                }),
            ),
        ] {
            let record = &repodata["v3"]["conda"][identifier.as_str()];
            assert_eq!(record["depends"], expected["depends"]);
            assert_eq!(record["constrains"], expected["constrains"]);
            assert_eq!(record["extra_depends"], expected["extra_depends"]);
            assert_eq!(
                repodata["packages"][legacy_filename]["depends"],
                serde_json::json!(["python >=3.10"])
            );
            assert_eq!(
                repodata["packages"][legacy_filename]["constrains"],
                serde_json::json!(["python <3.13"])
            );
            assert_eq!(
                repodata["packages"][legacy_filename]["extra_depends"],
                serde_json::json!({ "test": ["pytest >=8"] })
            );
        }
    }

    #[tokio::test]
    async fn invalid_patched_v3_dependency_is_rejected_before_any_repodata_write() {
        let channel = tempfile::tempdir().unwrap();
        let mut config = FsConfig::default();
        config.root = Some(channel.path().to_string_lossy().to_string());
        let op = Operator::new(config.into_builder()).unwrap().finish();
        op.create_dir("noarch/").await.unwrap();
        op.write("noarch/repodata_from_packages.json", "source sentinel")
            .await
            .unwrap();
        op.write("noarch/repodata.json", "published sentinel")
            .await
            .unwrap();

        let identifier = ArchiveIdentifier::from_str("v3-demo-1.0-0").unwrap();
        let mut repodata = RepoData {
            info: None,
            packages: IndexMap::default(),
            conda_packages: IndexMap::default(),
            v3: V3Packages::default(),
            removed: HashSet::default(),
            version: Some(1),
        };
        repodata.v3.conda.insert(
            identifier.clone(),
            PackageRecord::new(
                PackageName::new_unchecked("v3-demo"),
                Version::from_str("1.0").unwrap(),
                "0".to_string(),
            ),
        );
        let patch = serde_json::from_value::<PatchInstructions>(serde_json::json!({
            "v3": {
                "conda": {
                    (identifier.to_string()): { "depends": ["python[version="] }
                }
            }
        }))
        .unwrap();
        let metadata = RepodataMetadataCollection::new(
            &op,
            Platform::NoArch,
            true,
            false,
            false,
            PreconditionChecks::Disabled,
        )
        .await
        .unwrap();

        let error = write_repodata(repodata, Some(patch), Platform::NoArch, op, &metadata)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to parse depends MatchSpec")
        );
        assert_eq!(
            std::fs::read_to_string(channel.path().join("noarch/repodata_from_packages.json"))
                .unwrap(),
            "source sentinel"
        );
        assert_eq!(
            std::fs::read_to_string(channel.path().join("noarch/repodata.json")).unwrap(),
            "published sentinel"
        );
    }

    async fn index_empty_channel(base_url: Option<String>) -> serde_json::Value {
        let channel = tempfile::tempdir().unwrap();
        index_fs_with_channel_metadata(
            IndexFsConfig {
                channel: channel.path().to_path_buf(),
                target_platform: Some(Platform::NoArch),
                repodata_patch: None,
                write_zst: false,
                write_shards: false,
                repodata_revisions: Vec::new(),
                package_revision_assignment: PackageRevisionAssignment::default(),
                force: false,
                max_parallel: 1,
                multi_progress: None,
            },
            ChannelMetadata {
                base_url,
                ..ChannelMetadata::default()
            },
        )
        .await
        .unwrap();
        serde_json::from_slice(&std::fs::read(channel.path().join("noarch/repodata.json")).unwrap())
            .unwrap()
    }

    #[tokio::test]
    async fn indexer_writes_empty_v3_and_base_url_appropriate_repodata_version() {
        let without_base_url = index_empty_channel(None).await;
        let with_base_url = index_empty_channel(Some("../packages/".to_string())).await;

        for repodata in [&without_base_url, &with_base_url] {
            for map in ["packages", "packages.conda", "v3"] {
                assert_eq!(repodata[map], serde_json::json!({}), "missing {map}");
            }
        }
        assert_eq!(without_base_url["repodata_version"], 1);
        assert_eq!(with_base_url["repodata_version"], 2);
        assert_eq!(with_base_url["info"]["base_url"], "../packages/");
    }

    #[tokio::test]
    async fn indexer_preflights_unsupported_revisions_before_writing() {
        let channel = tempfile::tempdir().unwrap();
        let error = index_fs_with_channel_metadata(
            IndexFsConfig {
                channel: channel.path().to_path_buf(),
                target_platform: None,
                repodata_patch: None,
                write_zst: false,
                write_shards: false,
                repodata_revisions: vec![RepodataRevisionSelection {
                    revision: RepodataRevision::from(4),
                    message: None,
                }],
                package_revision_assignment: PackageRevisionAssignment::default(),
                force: false,
                max_parallel: 1,
                multi_progress: None,
            },
            ChannelMetadata::default(),
        )
        .await
        .unwrap_err();

        assert!(
            error.to_string().contains(
                "repodata revision v4 cannot be configured; only v3 is selectable and the legacy layout is implicit"
            )
        );
        assert!(!channel.path().join("noarch/repodata.json").exists());
    }

    #[test]
    fn indexer_only_produces_legacy_and_v3_package_layouts() {
        let filename = DistArchiveIdentifier::try_from_filename("demo-1.0-0.tar.bz2").unwrap();
        let indexed_record = |revision| IndexedPackageRecord {
            record: PackageRecord::new(
                PackageName::new_unchecked("demo"),
                Version::from_str("1.0").unwrap(),
                "0".to_string(),
            ),
            repodata_revision: revision,
            wheel_url: None,
        };

        let mut packages = IndexMap::default();
        let mut conda_packages = IndexMap::default();
        let mut v3 = V3Packages::default();
        insert_package_record_by_revision(
            &mut packages,
            &mut conda_packages,
            &mut v3,
            filename.clone(),
            indexed_record(RepodataRevision::Legacy),
            RepodataRevision::Legacy,
        )
        .unwrap();
        assert!(packages.contains_key(&filename));

        for revision in [
            RepodataRevision::Unknown(1),
            RepodataRevision::Unknown(2),
            RepodataRevision::Unknown(4),
        ] {
            let err = insert_package_record_by_revision(
                &mut IndexMap::default(),
                &mut IndexMap::default(),
                &mut V3Packages::default(),
                filename.clone(),
                indexed_record(revision),
                revision,
            )
            .unwrap_err();
            assert!(err.to_string().contains(&revision.to_string()));
        }
    }

    #[tokio::test]
    async fn indexer_canonicalizes_all_v3_buckets_in_zstd_and_shards_after_patching() {
        fn record(name: &str, dependency: &str) -> PackageRecord {
            let mut record = PackageRecord::new(
                PackageName::new_unchecked(name),
                Version::from_str("1.0").unwrap(),
                "0".to_string(),
            );
            record.depends = vec![dependency.to_string()];
            record.constrains = vec!["python <3.13".to_string()];
            record.extra_depends =
                BTreeMap::from([("test".to_string(), vec!["pytest >=8".to_string()])]);
            record
        }

        fn assert_canonical_dependencies(
            record: &serde_json::Value,
            python_version: &str,
            python_constraint: &str,
            pytest_version: &str,
        ) {
            assert_eq!(
                record["depends"],
                serde_json::json!([format!("python[version=\"{python_version}\"]")])
            );
            assert_eq!(
                record["constrains"],
                serde_json::json!([format!("python[version=\"{python_constraint}\"]")])
            );
            assert_eq!(
                record["extra_depends"],
                serde_json::json!({
                    "test": [format!("pytest[version=\"{pytest_version}\"]")]
                })
            );
        }

        let channel = tempfile::tempdir().unwrap();
        let mut config = FsConfig::default();
        config.root = Some(channel.path().to_string_lossy().to_string());
        let op = Operator::new(config.into_builder()).unwrap().finish();
        op.create_dir("noarch/").await.unwrap();

        let tar_identifier = ArchiveIdentifier::from_str("tar-demo-1.0-0").unwrap();
        let conda_identifier = ArchiveIdentifier::from_str("conda-demo-1.0-0").unwrap();
        let wheel_identifier = ArchiveIdentifier::from_str("wheel-demo-1.0-py_0").unwrap();
        let legacy_filename = "legacy-demo-1.0-0.tar.bz2";

        let mut repodata = RepoData {
            info: None,
            packages: IndexMap::default(),
            conda_packages: IndexMap::default(),
            v3: V3Packages::default(),
            removed: HashSet::default(),
            version: Some(1),
        };
        repodata.packages.insert(
            DistArchiveIdentifier::try_from_filename(legacy_filename).unwrap(),
            record("legacy-demo", "python >=3.10"),
        );
        repodata
            .v3
            .tar_bz2
            .insert(tar_identifier.clone(), record("tar-demo", "python >=3.10"));
        repodata.v3.conda.insert(
            conda_identifier.clone(),
            record("conda-demo", "python >=3.10"),
        );
        repodata.v3.whl.insert(
            wheel_identifier.clone(),
            WhlPackageRecord {
                package_record: record("wheel-demo", "python >=3.10"),
                url: UrlOrPath::Path("wheel-demo-1.0-py_0.whl".to_string()),
            },
        );
        repodata
            .v3
            .extensions
            .insert(
                "zip",
                serde_json::json!({
                    "metadata": { "keep": true, "remove": true }
                }),
            )
            .unwrap();

        let package_patch = serde_json::json!({
            "depends": ["python >=3.11"],
            "constrains": ["python <3.14"],
            "extra_depends": { "test": ["pytest >=9"] }
        });
        let mut tar_patches = serde_json::Map::new();
        tar_patches.insert(tar_identifier.to_string(), package_patch.clone());
        let mut conda_patches = serde_json::Map::new();
        conda_patches.insert(conda_identifier.to_string(), package_patch.clone());
        let mut wheel_patches = serde_json::Map::new();
        wheel_patches.insert(wheel_identifier.to_string(), package_patch);
        let patch: PatchInstructions = serde_json::from_value(serde_json::json!({
            "v3": {
                "tar.bz2": tar_patches,
                "conda": conda_patches,
                "whl": wheel_patches,
                "zip": {
                    "metadata": { "remove": null, "patched": true }
                }
            }
        }))
        .unwrap();

        let metadata = RepodataMetadataCollection::new(
            &op,
            Platform::NoArch,
            true,
            true,
            true,
            PreconditionChecks::Disabled,
        )
        .await
        .unwrap();
        write_repodata(repodata, Some(patch), Platform::NoArch, op, &metadata)
            .await
            .unwrap();

        let source: serde_json::Value = serde_json::from_slice(
            &std::fs::read(channel.path().join("noarch/repodata_from_packages.json")).unwrap(),
        )
        .unwrap();
        let published: serde_json::Value = serde_json::from_slice(
            &std::fs::read(channel.path().join("noarch/repodata.json")).unwrap(),
        )
        .unwrap();

        for (bucket, identifier) in [
            ("tar.bz2", tar_identifier.to_string()),
            ("conda", conda_identifier.to_string()),
            ("whl", wheel_identifier.to_string()),
        ] {
            assert_canonical_dependencies(
                &source["v3"][bucket][identifier.as_str()],
                ">=3.10",
                "<3.13",
                ">=8",
            );
            assert_canonical_dependencies(
                &published["v3"][bucket][identifier.as_str()],
                ">=3.11",
                "<3.14",
                ">=9",
            );
        }
        assert_eq!(
            source["packages"][legacy_filename]["depends"],
            serde_json::json!(["python >=3.10"])
        );
        assert_eq!(
            published["packages"][legacy_filename]["depends"],
            serde_json::json!(["python >=3.10"])
        );
        assert_eq!(
            published["v3"]["zip"],
            serde_json::json!({ "metadata": { "keep": true, "patched": true } })
        );

        let compressed: serde_json::Value = serde_json::from_slice(
            &zstd::stream::decode_all(
                std::fs::File::open(channel.path().join("noarch/repodata.json.zst")).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(compressed, published);

        let sharded: ShardedRepodata = rmp_serde::from_slice(
            &zstd::stream::decode_all(
                std::fs::File::open(channel.path().join("noarch/repodata_shards.msgpack.zst"))
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        for (package_name, bucket, identifier) in [
            ("tar-demo", "tar.bz2", tar_identifier.to_string()),
            ("conda-demo", "conda", conda_identifier.to_string()),
            ("wheel-demo", "whl", wheel_identifier.to_string()),
        ] {
            let digest = sharded.shards.get(package_name).unwrap();
            let shard: Shard = rmp_serde::from_slice(
                &zstd::stream::decode_all(
                    std::fs::File::open(
                        channel
                            .path()
                            .join("noarch/shards")
                            .join(format!("{}.msgpack.zst", hex::encode(digest))),
                    )
                    .unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
            let identifier = ArchiveIdentifier::from_str(&identifier).unwrap();
            let record = match bucket {
                "tar.bz2" => shard.v3.tar_bz2.get(&identifier).unwrap(),
                "conda" => shard.v3.conda.get(&identifier).unwrap(),
                "whl" => &shard.v3.whl.get(&identifier).unwrap().package_record,
                _ => unreachable!(),
            };
            assert_eq!(
                record.depends,
                vec!["python[version=\">=3.11\"]".to_string()]
            );
        }
    }

    #[tokio::test]
    async fn indexer_rejects_invalid_patched_v3_dependencies_before_writing() {
        let channel = tempfile::tempdir().unwrap();
        let mut config = FsConfig::default();
        config.root = Some(channel.path().to_string_lossy().to_string());
        let op = Operator::new(config.into_builder()).unwrap().finish();
        op.create_dir("noarch/").await.unwrap();

        let identifier = ArchiveIdentifier::from_str("demo-1.0-0").unwrap();
        let mut repodata = RepoData {
            info: None,
            packages: IndexMap::default(),
            conda_packages: IndexMap::default(),
            v3: V3Packages::default(),
            removed: HashSet::default(),
            version: Some(1),
        };
        repodata.v3.conda.insert(
            identifier.clone(),
            PackageRecord::new(
                PackageName::new_unchecked("demo"),
                Version::from_str("1.0").unwrap(),
                "0".to_string(),
            ),
        );
        let mut conda_patches = serde_json::Map::new();
        conda_patches.insert(
            identifier.to_string(),
            serde_json::json!({ "depends": ["python[extras=[Invalid]]"] }),
        );
        let patch: PatchInstructions = serde_json::from_value(serde_json::json!({
            "v3": { "conda": conda_patches }
        }))
        .unwrap();
        let metadata = RepodataMetadataCollection::new(
            &op,
            Platform::NoArch,
            true,
            true,
            true,
            PreconditionChecks::Disabled,
        )
        .await
        .unwrap();

        let error = write_repodata(repodata, Some(patch), Platform::NoArch, op, &metadata)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to parse depends MatchSpec 'python[extras=[Invalid]]'")
        );
        for path in [
            "noarch/repodata_from_packages.json",
            "noarch/repodata.json",
            "noarch/repodata.json.zst",
            "noarch/repodata_shards.msgpack.zst",
        ] {
            assert!(
                !channel.path().join(path).exists(),
                "unexpected write to {path}"
            );
        }
    }

    #[tokio::test]
    async fn indexer_rejects_legacy_extra_depends_patches_before_writing() {
        let channel = tempfile::tempdir().unwrap();
        let mut config = FsConfig::default();
        config.root = Some(channel.path().to_string_lossy().to_string());
        let op = Operator::new(config.into_builder()).unwrap().finish();
        op.create_dir("noarch/").await.unwrap();

        let patch: PatchInstructions = serde_json::from_value(serde_json::json!({
            "packages": {
                "demo-1.0-0.tar.bz2": {
                    "extra_depends": { "test": ["pytest >=8"] }
                }
            }
        }))
        .unwrap();
        let repodata = RepoData {
            info: None,
            packages: IndexMap::default(),
            conda_packages: IndexMap::default(),
            v3: V3Packages::default(),
            removed: HashSet::default(),
            version: Some(1),
        };
        let metadata = RepodataMetadataCollection::new(
            &op,
            Platform::NoArch,
            true,
            true,
            true,
            PreconditionChecks::Disabled,
        )
        .await
        .unwrap();

        let error = write_repodata(repodata, Some(patch), Platform::NoArch, op, &metadata)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("legacy repodata patches cannot set extra_depends")
        );
        for path in [
            "noarch/repodata_from_packages.json",
            "noarch/repodata.json",
            "noarch/repodata.json.zst",
            "noarch/repodata_shards.msgpack.zst",
        ] {
            assert!(
                !channel.path().join(path).exists(),
                "unexpected write to {path}"
            );
        }
    }

    #[tokio::test]
    async fn force_reindex_preserves_v3_extensions_and_recalculates_empty_stats() {
        let channel = tempfile::tempdir().unwrap();
        let noarch = channel.path().join("noarch");
        std::fs::create_dir(&noarch).unwrap();
        std::fs::write(
            noarch.join("repodata.json"),
            serde_json::to_vec(&serde_json::json!({
                "info": {
                    "subdir": "noarch",
                    "repodata_revisions": {
                        "v3": {
                            "message": "keep this message",
                            "n_packages": 99,
                            "oldest": 1,
                            "newest": 2
                        }
                    }
                },
                "packages": {},
                "packages.conda": {},
                "v3": { "zip": { "future": true } },
                "repodata_version": 1
            }))
            .unwrap(),
        )
        .unwrap();

        let mut config = FsConfig::default();
        config.root = Some(channel.path().to_string_lossy().to_string());
        let op = Operator::new(config.into_builder()).unwrap().finish();
        let stats = index(
            Some(Platform::NoArch),
            op,
            None,
            false,
            false,
            Vec::new(),
            PackageRevisionAssignment::default(),
            true,
            1,
            None,
            PreconditionChecks::Disabled,
        )
        .await
        .unwrap();

        assert_eq!(stats.subdirs[&Platform::NoArch].packages_added, 0);
        assert_eq!(stats.subdirs[&Platform::NoArch].packages_removed, 0);

        let repodata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(noarch.join("repodata.json")).unwrap()).unwrap();
        for map in ["packages", "packages.conda"] {
            assert_eq!(repodata[map], serde_json::json!({}), "missing {map}");
        }
        assert_eq!(repodata["v3"]["zip"], serde_json::json!({ "future": true }));
        assert_eq!(
            repodata["info"]["repodata_revisions"]["v3"]["message"],
            "keep this message"
        );
        assert_eq!(
            repodata["info"]["repodata_revisions"]["v3"]["n_packages"],
            0
        );
        assert!(
            repodata["info"]["repodata_revisions"]["v3"]
                .get("oldest")
                .is_none()
        );
        assert!(
            repodata["info"]["repodata_revisions"]["v3"]
                .get("newest")
                .is_none()
        );
    }
}
