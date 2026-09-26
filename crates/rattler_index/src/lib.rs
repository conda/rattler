//! Indexing of packages in a output folder to create up to date repodata.json
//! files
//!
//! Channel indexing assigns `indexed_timestamp` to newly published archives.
//! Existing publication timestamps, including legacy missing values, are
//! preserved when reindexing, even with `force` enabled. Archive readers do not
//! assign publication timestamps. Build timestamps in the future or later than
//! publication are flagged with warnings.
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
    ChannelInfo, ChannelNotice, ChannelNotices, ChannelRelations, MatchSpec, PackageRecord,
    ParseMatchSpecOptions, PatchInstructions, Platform, RepoData, Shard, ShardedRepodata,
    ShardedSubdirInfo, UrlOrPath, V3Extensions, V3Packages, WhlPackageRecord,
    package::{
        CondaArchiveType, DistArchiveIdentifier, DistArchiveType, IndexJson, PackageFile,
        RunExportsJson, ValidatedMatchSpecs, WheelArchiveType,
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
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
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
    /// Parsed `index.json` dependency specifications, when this record was
    /// read from a package archive. Existing repodata records do not retain
    /// this cache and are parsed only if they are emitted as v3.
    matchspecs: Option<ValidatedMatchSpecs>,
    wheel_url: Option<UrlOrPath>,
}

/// A package that could not be indexed.
///
/// The package is left out of the generated repodata; every other package in
/// the subdir is still indexed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedPackage {
    /// The filename of the package within its subdir.
    pub filename: String,
    /// Why the package could not be indexed.
    pub error: String,
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
    /// Packages that could not be read or parsed and were left out of the
    /// generated repodata.
    pub failed_packages: Vec<FailedPackage>,
    /// Number of packages that were not attempted because indexing was
    /// cancelled.
    pub packages_skipped: usize,
    /// Whether indexing of this subdir was cut short by cancellation.
    pub cancelled: bool,
    /// Whether the repodata files of this subdir were (re)written.
    ///
    /// This is `false` only when indexing was cancelled without publishing a
    /// partial result.
    pub repodata_written: bool,
}

/// Statistics for the entire indexing operation
#[derive(Debug, Clone, Default)]
pub struct IndexStats {
    /// Statistics per subdir
    pub subdirs: HashMap<Platform, SubdirIndexStats>,
    /// Whether indexing was cancelled before all subdirs were processed.
    pub cancelled: bool,
}

impl IndexStats {
    /// All packages that could not be indexed, with their subdir.
    pub fn failed_packages(&self) -> impl Iterator<Item = (Platform, &FailedPackage)> {
        self.subdirs.iter().flat_map(|(subdir, stats)| {
            stats
                .failed_packages
                .iter()
                .map(move |failed| (*subdir, failed))
        })
    }

    /// Whether any package could not be indexed.
    pub fn has_failures(&self) -> bool {
        self.subdirs
            .values()
            .any(|stats| !stats.failed_packages.is_empty())
    }
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

/// The raw metadata extracted from a package archive.
///
/// This is what the indexer stores in its cache: the unmodified
/// `info/index.json` and `info/run_exports.json` contents plus the archive
/// digests. A [`PackageRecord`] is derived from it with
/// [`ParsedPackage::to_package_record`], so cached packages go through exactly
/// the same derivation as freshly parsed ones.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParsedPackage {
    /// The raw contents of `info/index.json`.
    pub index_json: String,
    /// The parsed `info/run_exports.json`, if the archive contains one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_exports: Option<RunExportsJson>,
    /// Hex encoded SHA256 digest of the archive.
    pub sha256: String,
    /// Hex encoded MD5 digest of the archive.
    pub md5: String,
    /// Size of the archive in bytes.
    pub size: u64,
}

impl ParsedPackage {
    fn new(
        package_as_bytes: &[u8],
        index_json: String,
        run_exports: Option<RunExportsJson>,
    ) -> Self {
        let sha256 =
            rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(package_as_bytes);
        let md5 = rattler_digest::compute_bytes_digest::<rattler_digest::Md5>(package_as_bytes);
        Self {
            index_json,
            run_exports,
            sha256: hex::encode(sha256),
            md5: hex::encode(md5),
            size: package_as_bytes.len() as u64,
        }
    }

    /// Derive the package record from the raw package metadata.
    pub fn to_package_record(&self) -> std::io::Result<PackageRecord> {
        self.to_indexed_record().map(|indexed| indexed.record)
    }

    pub(crate) fn to_indexed_record(&self) -> std::io::Result<IndexedPackageRecord> {
        let validated = IndexJson::from_str(&self.index_json)?
            .into_validated()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        let repodata_revision = validated.required_repodata_revision();
        let (index, matchspecs) = validated.into_parts();

        let invalid_digest = |name: &str| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid {name} digest in parsed package metadata"),
            )
        };
        let sha256 = rattler_digest::parse_digest_from_hex::<rattler_digest::Sha256>(&self.sha256)
            .ok_or_else(|| invalid_digest("sha256"))?;
        let md5 = rattler_digest::parse_digest_from_hex::<rattler_digest::Md5>(&self.md5)
            .ok_or_else(|| invalid_digest("md5"))?;

        let package_record = PackageRecord {
            name: index.name,
            version: index.version,
            build: index.build,
            build_number: index.build_number,
            subdir: index.subdir.unwrap_or_else(|| "unknown".to_string()),
            md5: Some(md5),
            sha256: Some(sha256),
            size: Some(self.size),
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
            indexed_timestamp: None,
            python_site_packages_path: index.python_site_packages_path,
            legacy_bz2_md5: None,
            legacy_bz2_size: None,
            purls: index.purls,
            run_exports: self.run_exports.clone(),
            attestations_sha256: None,
        };

        Ok(IndexedPackageRecord {
            record: package_record,
            repodata_revision,
            matchspecs: Some(matchspecs),
            wheel_url: None,
        })
    }
}

/// Extract the package record from an `index.json` file.
pub fn package_record_from_index_json<T: Read>(
    package_as_bytes: impl AsRef<[u8]>,
    index_json_reader: &mut T,
) -> std::io::Result<PackageRecord> {
    let mut index_json = String::new();
    index_json_reader.read_to_string(&mut index_json)?;
    ParsedPackage::new(package_as_bytes.as_ref(), index_json, None).to_package_record()
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
    parse_tar_bz2_reader(reader)?.to_package_record()
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

/// Extract the package record from a `.conda` package file content.
/// This function will look for the `info/index.json` file in the conda package
/// and extract the package record from it.
pub fn package_record_from_conda_reader(reader: impl BufRead) -> std::io::Result<PackageRecord> {
    parse_conda_reader(reader)?.to_package_record()
}

/// Reads the whole reader into memory.
///
/// The archive readers need `Seek` (for `.conda`) and the digests need every
/// byte, so the package has to be buffered.
fn read_all(mut reader: impl BufRead) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Reads `info/index.json` (and `info/run_exports.json`) from the `info`
/// archive of a `.conda` package.
fn parse_info_archive(
    bytes: &[u8],
    archive: &mut tar::Archive<impl Read>,
) -> std::io::Result<ParsedPackage> {
    let mut index_json = None;
    let mut run_exports_json = None;
    for entry in archive.entries()?.flatten() {
        let mut entry = entry;
        let path = entry.path()?;
        if path.as_os_str().eq("info/index.json") {
            let mut contents = String::new();
            entry.read_to_string(&mut contents)?;
            index_json = Some(contents);
        } else if path.as_os_str().eq("info/run_exports.json") {
            run_exports_json = Some(RunExportsJson::from_reader(&mut entry)?);
        }
        if index_json.is_some() && run_exports_json.is_some() {
            break;
        }
    }

    let index_json = index_json.ok_or_else(|| std::io::Error::other("No index.json found"))?;
    Ok(ParsedPackage::new(bytes, index_json, run_exports_json))
}

fn parse_tar_bz2_reader(reader: impl BufRead) -> std::io::Result<ParsedPackage> {
    let bytes = read_all(reader)?;
    let mut archive = read::stream_tar_bz2(Cursor::new(&bytes));
    for entry in archive.entries()?.flatten() {
        let mut entry = entry;
        let path = entry.path()?;
        if path.as_os_str().eq("info/index.json") {
            let mut index_json = String::new();
            entry.read_to_string(&mut index_json)?;
            return Ok(ParsedPackage::new(&bytes, index_json, None));
        }
    }
    Err(std::io::Error::other("No index.json found"))
}

fn parse_conda_reader(reader: impl BufRead) -> std::io::Result<ParsedPackage> {
    let bytes = read_all(reader)?;
    let mut archive = seek::stream_conda_info(Cursor::new(&bytes))
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    parse_info_archive(&bytes, &mut archive)
}

/// Parse a package file buffer based on its filename extension.
///
/// # Arguments
///
/// * `buffer` - The file contents to parse
/// * `filename` - The filename (used to determine archive type)
fn parse_package_buffer(buffer: opendal::Buffer, filename: &str) -> std::io::Result<ParsedPackage> {
    let reader = buffer.reader();
    let archive_type = DistArchiveType::try_from(filename).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unsupported package archive: {filename}"),
        )
    })?;
    match archive_type {
        DistArchiveType::Conda(CondaArchiveType::TarBz2) => parse_tar_bz2_reader(reader),
        DistArchiveType::Conda(CondaArchiveType::Conda) => parse_conda_reader(reader),
        DistArchiveType::Wheel(WheelArchiveType::Whl) => Err(std::io::Error::other(
            "Package type \".whl\" not yet supported.",
        )),
    }
}

/// Limits the number of package bytes held in memory at once.
///
/// Permits are handed out in units of [`ByteBudget::UNIT`] bytes. A package
/// larger than the whole budget is clamped to the budget so it can still be
/// processed, on its own.
#[derive(Debug)]
pub(crate) struct ByteBudget {
    semaphore: Semaphore,
    max_units: u32,
}

impl ByteBudget {
    /// Granularity of a permit.
    const UNIT: u64 = 64 * 1024;

    pub(crate) fn new(max_bytes: u64) -> Self {
        let max_units = max_bytes
            .div_ceil(Self::UNIT)
            .clamp(1, u64::from(u32::MAX >> 3)) as u32;
        Self {
            semaphore: Semaphore::new(max_units as usize),
            max_units,
        }
    }

    pub(crate) async fn acquire(&self, bytes: u64) -> tokio::sync::SemaphorePermit<'_> {
        let units = bytes
            .div_ceil(Self::UNIT)
            .clamp(1, u64::from(self.max_units)) as u32;
        self.semaphore
            .acquire_many(units)
            .await
            .expect("byte budget semaphore was unexpectedly closed")
    }
}

/// Read and parse a package file with caching and retry logic.
///
/// This function encapsulates the logic for reading a package file, including:
/// - Checking the cache for a previously computed record
/// - Reading the file with retry logic on cache miss
/// - Parsing the package content on a blocking thread
/// - Storing the result (or the parse failure) in the cache
///
/// # Arguments
///
/// * `op` - The operator to use for file operations
/// * `cache` - The package record cache (scoped to a single subdir)
/// * `subdir` - The subdirectory (e.g., "noarch", "linux-64")
/// * `filename` - The package filename (e.g., "package-1.0.0.tar.bz2")
/// * `byte_budget` - Optional limit on the package bytes in memory at once
///
/// # Returns
///
/// Returns the parsed package record on success.
async fn read_and_parse_package(
    op: &Operator,
    cache: &cache::PackageRecordCache,
    subdir: Platform,
    filename: &str,
    byte_budget: Option<&ByteBudget>,
) -> std::io::Result<IndexedPackageRecord> {
    let file_path = format!("{subdir}/{filename}");

    // Try cache or get current metadata
    let current_metadata = match cache.get_or_stat(op, &file_path).await {
        Ok(cache::CacheResult::Hit(package)) => return package.to_indexed_record(),
        Ok(cache::CacheResult::Broken(error)) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{error} (cached failure, the file has not changed since)"),
            ));
        }
        Ok(cache::CacheResult::Miss(metadata)) => Some(metadata),
        Err(e) => {
            tracing::warn!("Cache stat failed for {file_path}: {e}, proceeding without cache");
            None
        }
    };

    // Reserve memory for the package before downloading it. The permit is held
    // until the parse finished and the buffer is dropped.
    let _permit = match byte_budget {
        Some(budget) => Some(
            budget
                .acquire(current_metadata.as_ref().and_then(|m| m.size).unwrap_or(0))
                .await,
        ),
        None => None,
    };

    let (buffer, final_metadata) = if let Some(metadata) = &current_metadata {
        let (buffer, final_metadata) = cache::read_package_with_retry(
            op,
            &file_path,
            RepodataFileMetadata {
                etag: metadata.etag.clone(),
                last_modified: metadata.last_modified,
                file_existed: true, // File exists since we got its metadata from stat
                precondition_checks: PreconditionChecks::Enabled, // Always enabled for cache reads
            },
        )
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;
        let final_metadata = cache::CachedFileMetadata {
            etag: final_metadata.etag,
            last_modified: final_metadata.last_modified,
            size: Some(buffer.len() as u64),
        };
        (buffer, Some(final_metadata))
    } else {
        // Fall back to direct read without cache
        let buffer = op
            .read(&file_path)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        (buffer, None)
    };

    // Decompressing and hashing is CPU bound; keep it off the async workers.
    // A panic inside the parser is reported as a failure of this package only.
    let owned_filename = filename.to_owned();
    let parsed = tokio::task::spawn_blocking(move || parse_package_buffer(buffer, &owned_filename))
        .await
        .unwrap_or_else(|join_error| {
            Err(std::io::Error::other(format!(
                "parsing the package panicked: {join_error}"
            )))
        });

    match parsed {
        Ok(parsed) => {
            let parsed = Arc::new(parsed);
            if let Some(final_metadata) = final_metadata {
                cache
                    .insert(&file_path, parsed.clone(), final_metadata)
                    .await;
            }
            parsed.to_indexed_record()
        }
        Err(err) => {
            if let Some(final_metadata) = final_metadata {
                cache
                    .insert_broken(&file_path, err.to_string(), final_metadata)
                    .await;
            }
            Err(err)
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

/// Everything needed to index one subdir.
#[derive(Clone)]
struct SubdirIndexParams {
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
    max_parallel: usize,
    byte_budget: Option<Arc<ByteBudget>>,
    cache: cache::PackageRecordCache,
    precondition_checks: PreconditionChecks,
    cancellation_token: CancellationToken,
    publish_partial: bool,
}

async fn index_subdir(params: SubdirIndexParams) -> Result<SubdirIndexStats, RepodataError> {
    // Use write_retry_policy for handling lock contention during repodata writes
    // This will retry for 10 minutes with longer backoff durations (10s, 30s, 60s, etc.)
    let retry_policy = write_retry_policy();
    let mut current_try = 0;
    let subdir = params.subdir;

    loop {
        let request_start_time = SystemTime::now();

        match index_subdir_inner(params.clone()).await {
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

                // Never retry once cancellation was requested.
                if is_retryable_condition_error && !params.cancellation_token.is_cancelled() {
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
                            tokio::select! {
                                () = tokio::time::sleep(duration) => {}
                                () = params.cancellation_token.cancelled() => {
                                    tracing::info!("Cancelled while waiting to retry {subdir}");
                                    return Err(e);
                                }
                            }
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

async fn index_subdir_inner(params: SubdirIndexParams) -> Result<SubdirIndexStats, RepodataError> {
    let SubdirIndexParams {
        subdir,
        op,
        force,
        write_zst,
        write_shards,
        repodata_revisions,
        package_revision_assignment,
        channel_metadata,
        repodata_patch,
        progress,
        max_parallel,
        byte_budget,
        cache,
        precondition_checks,
        cancellation_token,
        publish_partial,
    } = params;

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
    // Keep publication history separate from reusable archive metadata. A
    // present entry with no timestamp is a legacy publication, not a new one.
    // Include raw records (patches may remove packages), but let published
    // records win, including intentional corrections and missing values.
    let mut indexed_timestamps: HashMap<_, _> = package_source
        .packages
        .iter()
        .map(|(filename, package)| (filename.clone(), package.record.indexed_timestamp))
        .collect();
    let existing_repodata = if repodata_patch.is_some() {
        let published =
            read_existing_repodata(&op, &format!("{subdir}/{REPODATA}"), &metadata.repodata)
                .await?;
        indexed_timestamps.extend(
            published
                .packages
                .iter()
                .map(|(filename, package)| (filename.clone(), package.record.indexed_timestamp)),
        );
        merge_patch_repodata(package_source, published)
    } else {
        // When patches are disabled, packages previously hidden by a removal
        // patch may only have publication history in the old raw repodata.
        let raw_path = format!("{subdir}/{REPODATA_FROM_PACKAGES}");
        let raw_metadata = RepodataFileMetadata::new(&op, &raw_path, precondition_checks).await?;
        let raw = read_existing_repodata(&op, &raw_path, &raw_metadata).await?;
        for (filename, package) in raw.packages {
            indexed_timestamps
                .entry(filename)
                .or_insert(package.record.indexed_timestamp);
        }
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

    // List all the packages in the subdirectory. Attestation sidecars
    // (`<package>.sigs`) are collected alongside so that the corresponding
    // package records can advertise them.
    let mut uploaded_packages: HashSet<DistArchiveIdentifier> = HashSet::new();
    let mut attestation_sidecars: HashSet<DistArchiveIdentifier> = HashSet::new();
    for entry in op.list_with(&format!("{}/", subdir.as_str())).await? {
        if !entry.metadata().mode().is_file() {
            continue;
        }
        let filename = entry.name();
        if let Some(package_filename) = filename.strip_suffix(ATTESTATION_SIDECAR_SUFFIX) {
            if let Some(identifier) = DistArchiveIdentifier::try_from_filename(package_filename) {
                attestation_sidecars.insert(identifier);
            }
        } else if let Some(identifier) = DistArchiveIdentifier::try_from_filename(filename) {
            uploaded_packages.insert(identifier);
        }
    }

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

    // Deterministic order so that interrupted runs make visible progress
    // through the same list.
    let mut packages_to_add = uploaded_packages
        .difference(&registered_packages.keys().cloned().collect::<HashSet<_>>())
        .cloned()
        .collect::<Vec<_>>();
    packages_to_add.sort_by_cached_key(DistArchiveIdentifier::to_file_name);

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

    // Packages are processed through a lazy stream: at most `max_parallel`
    // packages are in flight and no work is created for packages that have not
    // been reached yet. Once cancellation is requested no new package is
    // started; the ones in flight run to completion so their result lands in
    // the cache.
    let total_to_add = packages_to_add.len();
    let mut results = Vec::new();
    let mut failed_packages = Vec::new();
    let mut attempted = 0usize;
    {
        let op = &op;
        let cache = &cache;
        let pb = &pb;
        let byte_budget = byte_budget.as_deref();
        let cancellation_token = &cancellation_token;
        let mut stream = futures::stream::iter(packages_to_add)
            .take_while(|_| futures::future::ready(!cancellation_token.is_cancelled()))
            .map(|filename| async move {
                pb.set_message(format!(
                    "Indexing {} {}",
                    subdir.as_str(),
                    console::style(filename.to_file_name()).dim()
                ));
                let result = read_and_parse_package(
                    op,
                    cache,
                    subdir,
                    &filename.to_file_name(),
                    byte_budget,
                )
                .await;
                pb.inc(1);
                (filename, result)
            })
            .buffer_unordered(max_parallel.max(1));

        while let Some((filename, result)) = stream.next().await {
            attempted += 1;
            match result {
                Ok(record) => results.push((filename, record)),
                Err(error) => {
                    let filename = filename.to_file_name();
                    tracing::error!("Failed to index {subdir}/{filename}: {error}");
                    failed_packages.push(FailedPackage {
                        filename,
                        error: error.to_string(),
                    });
                }
            }
        }
    }
    let cancelled = cancellation_token.is_cancelled() && attempted < total_to_add;
    let packages_skipped = total_to_add - attempted;
    failed_packages.sort_by(|a, b| a.filename.cmp(&b.filename));

    if cancelled {
        pb.abandon_with_message(format!(
            "{} {} ({} packages not indexed)",
            console::style("Interrupted").yellow(),
            subdir.as_str(),
            packages_skipped
        ));
    } else if failed_packages.is_empty() {
        pb.finish_with_message(format!(
            "{} {}",
            console::style("Finished").green(),
            subdir.as_str()
        ));
    } else {
        pb.finish_with_message(format!(
            "{} {} ({} packages failed)",
            console::style("Finished").yellow(),
            subdir.as_str(),
            failed_packages.len()
        ));
    }

    tracing::info!(
        "Successfully added {} packages to subdir {} ({} failed, {} skipped).",
        results.len(),
        subdir,
        failed_packages.len(),
        packages_skipped
    );

    // Persist what was learned about this subdir, dropping entries for files
    // that no longer exist in the channel.
    let known_paths = uploaded_packages
        .iter()
        .map(|identifier| format!("{subdir}/{}", identifier.to_file_name()))
        .collect::<HashSet<_>>();
    if let Err(err) = cache.compact(&known_paths).await {
        tracing::warn!("Failed to compact the package cache for {subdir}: {err}");
    }

    let stats = SubdirIndexStats {
        packages_added: results.len(),
        packages_removed: packages_to_delete.len(),
        retries: 0, // Will be set by index_subdir
        failed_packages,
        packages_skipped,
        cancelled,
        repodata_written: false,
    };

    if cancelled && !publish_partial {
        tracing::info!(
            "Indexing of {subdir} was cancelled; not writing repodata. \
             Re-run to continue from the cache."
        );
        return Ok(stats);
    }

    for (filename, record) in results {
        registered_packages.insert(filename, record);
    }

    apply_attestation_sidecars(&op, subdir, &attestation_sidecars, &mut registered_packages)
        .await?;

    let mut packages: IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState> =
        IndexMap::default();
    let mut conda_packages: IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState> =
        IndexMap::default();
    let mut v3 = V3Packages {
        extensions: v3_extensions,
        ..V3Packages::default()
    };
    let latest_revision = latest_repodata_revision(&repodata_revisions);
    let indexed_at = jiff::Timestamp::now().into();
    for (filename, mut package) in registered_packages {
        // Assign only after extraction/cache lookup, using the history freshly
        // read on this attempt. A concurrent publisher's timestamp therefore
        // takes precedence over any work performed before a retry.
        package.record.indexed_timestamp = indexed_timestamps
            .get(&filename)
            .copied()
            .unwrap_or(Some(indexed_at));
        if let Some(timestamp) = package.record.timestamp {
            if timestamp > indexed_at {
                tracing::warn!(%filename, %subdir, ?timestamp, "Package build timestamp is in the future");
            }
            if let Some(indexed_timestamp) = package.record.indexed_timestamp
                && timestamp > indexed_timestamp
            {
                tracing::warn!(%filename, %subdir, ?timestamp, ?indexed_timestamp, "Package build timestamp is later than its indexed timestamp");
            }
        }
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
    let repodata_before_patches = RepoData {
        info: Some(ChannelInfo {
            subdir: Some(subdir.to_string()),
            base_url: channel_metadata.base_url,
            repodata_revisions: repodata_revisions_for_packages(
                &repodata_revisions,
                &existing_repodata_revisions,
                &v3,
            ),
            channel_relations: channel_metadata.channel_relations,
        }),
        packages,
        conda_packages,
        v3,
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
        repodata_written: true,
        ..stats
    })
}

/// The suffix of an attestation sidecar file that lives next to a package in a
/// channel: `<package_filename>.sigs`.
pub const ATTESTATION_SIDECAR_SUFFIX: &str = ".sigs";

/// Reads the attestation sidecar (`<package>.sigs`) for every registered
/// package that has one, records its SHA256 in the package record and verifies
/// that the identical content-addressed sidecar (`<package>.sigs.<sha256>`)
/// has already been published.
///
/// Packages without a sidecar have `attestations_sha256` cleared so that a
/// removed sidecar is no longer advertised.
async fn apply_attestation_sidecars(
    op: &Operator,
    subdir: Platform,
    sidecars: &HashSet<DistArchiveIdentifier>,
    registered_packages: &mut ahash::HashMap<DistArchiveIdentifier, IndexedPackageRecord>,
) -> Result<(), RepodataError> {
    for (identifier, indexed) in registered_packages.iter_mut() {
        if !sidecars.contains(identifier) {
            indexed.record.attestations_sha256 = None;
            continue;
        }

        let package_filename = identifier.to_file_name();
        let sidecar_path = format!("{subdir}/{package_filename}{ATTESTATION_SIDECAR_SUFFIX}");
        let bytes = op.read(&sidecar_path).await?.to_bytes();

        // The sidecar must be a JSON array of Sigstore bundles.
        match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(serde_json::Value::Array(bundles)) if !bundles.is_empty() => {}
            Ok(_) => {
                return Err(RepodataError::Other(anyhow::anyhow!(
                    "attestation sidecar {sidecar_path} must be a non-empty JSON array of Sigstore bundles"
                )));
            }
            Err(err) => {
                return Err(RepodataError::Other(anyhow::anyhow!(
                    "attestation sidecar {sidecar_path} is not valid JSON: {err}"
                )));
            }
        }

        let sha256 = rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(&bytes);
        let content_addressed_path = format!("{sidecar_path}.{}", hex::encode(sha256));
        let content_addressed_bytes = op
            .read(&content_addressed_path)
            .await
            .map_err(|err| {
                RepodataError::Other(anyhow::anyhow!(
                    "failed to read content-addressed attestation sidecar {content_addressed_path}: {err}"
                ))
            })?
            .to_bytes();
        if content_addressed_bytes != bytes {
            return Err(RepodataError::Other(anyhow::anyhow!(
                "content-addressed attestation sidecar {content_addressed_path} does not match {sidecar_path}"
            )));
        }

        indexed.record.attestations_sha256 = Some(sha256);
    }
    Ok(())
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
                        matchspecs: None,
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
                matchspecs: None,
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

/// Renders package dependency fields for their destination repodata revision.
///
/// Fresh package archives retain their parsed `MatchSpecs` from `index.json`
/// validation. Records read from existing repodata are parsed here before they
/// are re-emitted, so patches and layout migrations cannot publish syntax that
/// the target revision cannot represent.
fn render_record_matchspecs_for_revision(
    record: &mut PackageRecord,
    matchspecs: Option<ValidatedMatchSpecs>,
    revision: RepodataRevision,
) -> Result<(), RepodataError> {
    if revision.uses_legacy_package_layout() && !record.flags.is_empty() {
        return Err(RepodataError::Other(anyhow::anyhow!(
            "legacy repodata cannot represent package flags"
        )));
    }

    let rendered: anyhow::Result<rattler_conda_types::package::RenderedMatchSpecs> =
        if let Some(matchspecs) = matchspecs {
            matchspecs
                .render_for_revision(revision)
                .map_err(anyhow::Error::from)
        } else {
            let parse_options =
                ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3);
            let render = |field: &str, spec: &str, allow_v3: bool| -> anyhow::Result<String> {
                let parsed = MatchSpec::from_str(spec, parse_options).with_context(|| {
                    format!("failed to parse {revision} repodata MatchSpec in {field}: '{spec}'")
                })?;
                if revision.uses_legacy_package_layout()
                    && !allow_v3
                    && !parsed
                        .required_repodata_revision()
                        .uses_legacy_package_layout()
                {
                    anyhow::bail!(
                        "legacy repodata cannot represent MatchSpec in {field}: '{spec}'"
                    );
                }
                if revision.as_u64() >= RepodataRevision::V3.as_u64() {
                    parsed.to_canonical_string().map_err(anyhow::Error::from)
                } else {
                    Ok(parsed.to_string())
                }
            };
            Ok(rattler_conda_types::package::RenderedMatchSpecs {
                depends: record
                    .depends
                    .iter()
                    .map(|spec| render("depends", spec, false))
                    .collect::<Result<_, _>>()?,
                constrains: record
                    .constrains
                    .iter()
                    .map(|spec| render("constrains", spec, false))
                    .collect::<Result<_, _>>()?,
                extra_depends: record
                    .extra_depends
                    .iter()
                    .map(|(group, specs)| {
                        specs
                            .iter()
                            .map(|spec| render(&format!("extra_depends.{group}"), spec, true))
                            .collect::<Result<_, _>>()
                            .map(|rendered| (group.clone(), rendered))
                    })
                    .collect::<Result<_, _>>()?,
            })
        };
    let rendered = rendered?;

    record.depends = rendered.depends;
    record.constrains = rendered.constrains;
    record.extra_depends = rendered.extra_depends;
    Ok(())
}

/// Validates legacy records and canonicalizes v3 dependency `MatchSpecs`.
///
/// Patches apply after initial package indexing and can replace dependency
/// strings, so this is deliberately run immediately before writing repodata
/// (and before deriving its shards).
fn validate_repodata_matchspecs(repodata: &mut RepoData) -> Result<(), RepodataError> {
    for record in repodata.packages.values_mut() {
        render_record_matchspecs_for_revision(record, None, RepodataRevision::Legacy)?;
    }
    for record in repodata.conda_packages.values_mut() {
        render_record_matchspecs_for_revision(record, None, RepodataRevision::Legacy)?;
    }
    for record in repodata.v3.tar_bz2.values_mut() {
        render_record_matchspecs_for_revision(record, None, RepodataRevision::V3)?;
    }
    for record in repodata.v3.conda.values_mut() {
        render_record_matchspecs_for_revision(record, None, RepodataRevision::V3)?;
    }
    for record in repodata.v3.whl.values_mut() {
        render_record_matchspecs_for_revision(
            &mut record.package_record,
            None,
            RepodataRevision::V3,
        )?;
    }
    Ok(())
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
    if package.repodata_revision.as_u64() > RepodataRevision::V3.as_u64()
        && revision.as_u64() < package.repodata_revision.as_u64()
    {
        return Err(RepodataError::Other(anyhow::anyhow!(
            "package requires repodata revision {}, but the effective index revision is {}",
            package.repodata_revision,
            revision
        )));
    }

    let IndexedPackageRecord {
        mut record,
        wheel_url,
        matchspecs,
        ..
    } = package;

    if revision.uses_legacy_package_layout() {
        // Reparse records placed in legacy maps so v3-origin metadata is
        // checked against the legacy feature set before it is emitted.
        render_record_matchspecs_for_revision(&mut record, None, revision)?;
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
        render_record_matchspecs_for_revision(&mut record, matchspecs, revision)?;
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
        if let Some(timestamp) = record.timestamp {
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
    v3: &V3Packages,
) -> RepodataRevisions {
    // `BTreeMap` keeps the result ordered ascending regardless of input order.
    // Existing package statistics are deliberately discarded: generated
    // statistics always describe the typed records written below.
    //
    // The legacy layout is never advertised. CEP 48 requires every
    // `info.repodata_revisions` key to be `vN` with `N` >= 3, so there is no
    // revision to describe the `packages` and `packages.conda` maps, and an
    // existing `v0` entry is dropped rather than carried forward.
    let mut revisions = existing
        .iter()
        .filter(|(revision, _)| **revision == RepodataRevision::V3)
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

/// Write a `repodata.json` for all packages in the given configurator's root.
/// Uses conditional writes based on the provided metadata to prevent concurrent
/// modification issues.
pub async fn write_repodata(
    repodata: RepoData,
    repodata_patch: Option<PatchInstructions>,
    subdir: Platform,
    op: Operator,
    metadata: &RepodataMetadataCollection,
) -> Result<(), RepodataError> {
    if let Some(repodata_from_packages_metadata) = &metadata.repodata_from_packages {
        let unpatched_repodata_path = format!("{subdir}/{REPODATA_FROM_PACKAGES}");
        tracing::info!("Writing unpatched repodata to {unpatched_repodata_path}");
        let unpatched_repodata_bytes = serde_json::to_vec(&repodata)?;
        crate::utils::write_with_metadata_check(
            &op,
            &unpatched_repodata_path,
            unpatched_repodata_bytes,
            repodata_from_packages_metadata,
            Some(CACHE_CONTROL_REPODATA),
        )
        .await?;
    }

    let mut repodata = if let Some(instructions) = repodata_patch {
        tracing::info!("Patching repodata");
        let mut patched_repodata = repodata.clone();
        patched_repodata.apply_patches(&instructions);
        patched_repodata
    } else {
        repodata
    };
    validate_repodata_matchspecs(&mut repodata)?;

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

/// Options that control how packages are processed, shared by all backends.
///
/// These are the knobs that matter for large channels: where parsed package
/// metadata is persisted, how much memory may be used for packages in flight,
/// and how to react to cancellation.
#[derive(Debug, Clone, Default)]
pub struct IndexProcessingOptions {
    /// Directory in which parsed package metadata is cached across runs, one
    /// file per subdir. When set, a package is only downloaded and parsed
    /// again if its `ETag`, modification time or size changed, and packages
    /// that could not be parsed are not retried until the file changes.
    ///
    /// This is what makes an interrupted run resumable: everything parsed so
    /// far is on disk and the next run continues from there.
    ///
    /// The directory must not be shared by processes that index at the same
    /// time; give each concurrent indexer its own directory.
    ///
    /// `None` keeps parsed metadata in memory only.
    pub cache_dir: Option<PathBuf>,
    /// Upper bound on the package bytes held in memory at once.
    ///
    /// `None` only limits the number of packages in flight (`max_parallel`).
    pub max_in_flight_bytes: Option<u64>,
    /// Token to request cooperative cancellation. Once cancelled, no new
    /// package is started; packages in flight finish and are cached. Subdirs
    /// that were not reached are skipped.
    pub cancellation_token: Option<CancellationToken>,
    /// Whether to write repodata for the packages collected so far when
    /// indexing of a subdir is cancelled. If `false` (the default) the
    /// repodata of a cancelled subdir is left untouched and only the cache is
    /// updated.
    pub publish_partial: bool,
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
    /// Caching, memory and cancellation options.
    pub processing: IndexProcessingOptions,
}

/// Create a new `repodata.json` for all packages in the channel at the given
/// directory.
pub async fn index_fs(config: IndexFsConfig) -> anyhow::Result<IndexStats> {
    index_fs_with_channel_metadata(config, ChannelMetadata::default()).await
}

/// Create a new `repodata.json` for all packages in the channel at the given
/// directory and write channel metadata into the generated repodata.
///
/// Packages that cannot be read or parsed are left out of the repodata and
/// reported in the returned [`IndexStats`]; check [`IndexStats::has_failures`].
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
        processing,
    }: IndexFsConfig,
    channel_metadata: ChannelMetadata,
) -> anyhow::Result<IndexStats> {
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
    index_with_options(
        op,
        IndexOptions {
            target_platform,
            repodata_patch,
            write_zst,
            write_shards,
            repodata_revisions,
            package_revision_assignment,
            force,
            max_parallel,
            multi_progress,
            precondition_checks: PreconditionChecks::Disabled,
            channel_metadata,
            processing,
        },
    )
    .await
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
    /// Caching, memory and cancellation options.
    pub processing: IndexProcessingOptions,
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
pub async fn index_s3(config: IndexS3Config) -> anyhow::Result<IndexStats> {
    index_s3_with_channel_metadata(config, ChannelMetadata::default()).await
}

/// Create a new `repodata.json` for all packages in the channel at the given S3
/// URL and write channel metadata into the generated repodata.
///
/// Packages that cannot be read or parsed are left out of the repodata and
/// reported in the returned [`IndexStats`]; check [`IndexStats::has_failures`].
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
        processing,
    }: IndexS3Config,
    channel_metadata: ChannelMetadata,
) -> anyhow::Result<IndexStats> {
    // Create the S3 configuration for opendal.
    let s3_config = s3_config(&credentials, &channel)?;
    let builder = s3_config.into_builder();
    let op = Operator::new(builder)?.layer(RetryLayer::new()).finish();

    index_with_options(
        op,
        IndexOptions {
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
            channel_metadata,
            processing,
        },
    )
    .await
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
///
/// See [`index_with_options`] for caching, memory and cancellation controls.
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
///
/// See [`index_with_options`] for caching, memory and cancellation controls.
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
    index_with_options(
        op,
        IndexOptions {
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
            channel_metadata,
            processing: IndexProcessingOptions::default(),
        },
    )
    .await
}

/// All options of [`index_with_options`].
#[derive(Debug, Clone, Default)]
pub struct IndexOptions {
    /// The target platform to index. `None` indexes every subdir found.
    pub target_platform: Option<Platform>,
    /// The filename of a repodata patch package in `noarch` to apply.
    pub repodata_patch: Option<String>,
    /// Whether to write the repodata as a zstd-compressed file.
    pub write_zst: bool,
    /// Whether to write the repodata shards.
    pub write_shards: bool,
    /// Repodata revisions to advertise in generated repodata.
    pub repodata_revisions: Vec<RepodataRevisionSelection>,
    /// How packages are assigned to repodata revisions.
    pub package_revision_assignment: PackageRevisionAssignment,
    /// Whether to ignore the records of the existing repodata.
    pub force: bool,
    /// The maximum number of packages processed in parallel.
    pub max_parallel: usize,
    /// The multi-progress bar to use for the index.
    pub multi_progress: Option<MultiProgress>,
    /// Configuration for precondition checks during file operations.
    pub precondition_checks: PreconditionChecks,
    /// Metadata written into the generated repodata.
    pub channel_metadata: ChannelMetadata,
    /// Caching, memory and cancellation options.
    pub processing: IndexProcessingOptions,
}

/// Create a new `repodata.json` for all packages in the given operator's root.
///
/// This is the most general entry point; [`index`], [`index_with_channel_metadata`],
/// [`index_fs`] and [`index_s3`] all delegate to it.
///
/// Packages that cannot be read or parsed do not abort indexing. They are left
/// out of the generated repodata and listed in
/// [`SubdirIndexStats::failed_packages`]. Subdir-level failures (listing the
/// subdir, reading or writing repodata) are still returned as errors.
///
/// When the [`IndexProcessingOptions::cancellation_token`] is cancelled, no new
/// package is started, packages in flight finish, and
/// [`IndexStats::cancelled`] is set. Combined with a
/// [`IndexProcessingOptions::cache_dir`], the next run continues where this
/// one stopped.
pub async fn index_with_options(op: Operator, options: IndexOptions) -> anyhow::Result<IndexStats> {
    let IndexOptions {
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
        channel_metadata,
        processing,
    } = options;
    validate_configured_repodata_revisions(&repodata_revisions)?;
    let cancellation_token = processing.cancellation_token.unwrap_or_default();

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
        Some(repodata_patch)
    } else {
        None
    };

    let byte_budget = processing
        .max_in_flight_bytes
        .map(|bytes| Arc::new(ByteBudget::new(bytes)));

    // Deterministic subdir order so an interrupted run resumes predictably.
    let mut subdirs = subdirs.into_iter().collect::<Vec<_>>();
    subdirs.sort_by_key(|subdir| subdir.as_str());

    let mut stats = IndexStats {
        subdirs: HashMap::new(),
        cancelled: false,
    };

    for subdir in subdirs {
        if cancellation_token.is_cancelled() {
            tracing::info!("Indexing cancelled; skipping subdir {subdir}");
            stats.cancelled = true;
            continue;
        }

        // Create a separate cache for each subdir.
        // The cache persists across retry attempts for this specific subdir
        // and, with a cache directory, across runs.
        let cache = match &processing.cache_dir {
            Some(cache_dir) => cache::PackageRecordCache::with_store(
                cache_dir.join(format!("{}.jsonl", subdir.as_str())),
            )
            .with_context(|| format!("failed to open the package cache for {subdir}"))?,
            None => cache::PackageRecordCache::new(),
        };

        let task = index_subdir(SubdirIndexParams {
            subdir,
            op: op.clone(),
            force,
            write_zst,
            write_shards,
            repodata_revisions: repodata_revisions.clone(),
            package_revision_assignment,
            channel_metadata: channel_metadata.clone(),
            repodata_patch: repodata_patch
                .as_ref()
                .and_then(|p| p.subdirs.get(&subdir.to_string()).cloned()),
            progress: multi_progress.clone(),
            max_parallel,
            byte_budget: byte_budget.clone(),
            cache,
            precondition_checks,
            cancellation_token: cancellation_token.clone(),
            publish_partial: processing.publish_partial,
        })
        .instrument(tracing::info_span!("index_subdir", subdir = %subdir));

        match task.await {
            Ok(subdir_stats) => {
                stats.cancelled |= subdir_stats.cancelled;
                stats.subdirs.insert(subdir, subdir_stats);
            }
            Err(e) => {
                tracing::error!("Failed to process subdir: {e}");
                return Err(e.into());
            }
        }
    }

    // Publish notices only after all repodata updates succeeded, so a failed
    // or interrupted indexing operation cannot partially update channel-level
    // messaging.
    if !stats.cancelled
        && let (Some(notices), Some(metadata)) =
            (&channel_metadata.notices, notices_metadata.as_ref())
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
        version: Some(2),
    };

    let repodata_bytes = serde_json::to_vec(&empty_repodata)?;
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
    use std::str::FromStr;

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
    fn legacy_packages_are_not_advertised_as_a_revision() {
        // CEP 48 has no revision key for the legacy package maps, so indexing a
        // channel that only holds them must not write `info.repodata_revisions`,
        // and an existing `v0` entry must not survive a re-index.
        let existing = RepodataRevisions::from([(
            RepodataRevision::Legacy,
            RepodataRevisionMetadata {
                message: Some("stale legacy message".to_string()),
                n_packages: Some(99),
                ..RepodataRevisionMetadata::default()
            },
        )]);

        let revisions = repodata_revisions_for_packages(&[], &existing, &V3Packages::default());

        assert!(revisions.is_empty());
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
        let preserved = repodata_revisions_for_packages(&[], &existing, &V3Packages::default());
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
            &V3Packages::default(),
        );
        assert_eq!(
            overridden[&RepodataRevision::V3].message.as_deref(),
            Some("caller message")
        );
    }

    #[test]
    fn latest_assignment_rejects_future_revision_demotion() {
        let filename = DistArchiveIdentifier::try_from_filename("demo-1.0-0.tar.bz2").unwrap();
        let indexed = IndexedPackageRecord {
            record: PackageRecord::new(
                PackageName::new_unchecked("demo"),
                Version::from_str("1.0").unwrap(),
                "0".to_string(),
            ),
            repodata_revision: RepodataRevision::from(4),
            matchspecs: None,
            wheel_url: None,
        };
        let error = insert_package_record_by_revision(
            &mut IndexMap::default(),
            &mut IndexMap::default(),
            &mut V3Packages::default(),
            filename,
            indexed,
            RepodataRevision::V3,
        )
        .unwrap_err();

        assert!(error.to_string().contains("requires repodata revision v4"));
    }

    #[test]
    fn legacy_representable_v3_record_can_be_downleveled() {
        let filename = DistArchiveIdentifier::try_from_filename("demo-1.0-0.tar.bz2").unwrap();
        let indexed = IndexedPackageRecord {
            record: PackageRecord::new(
                PackageName::new_unchecked("demo"),
                Version::from_str("1.0").unwrap(),
                "0".to_string(),
            ),
            repodata_revision: RepodataRevision::V3,
            matchspecs: None,
            wheel_url: None,
        };
        let mut packages = IndexMap::default();
        insert_package_record_by_revision(
            &mut packages,
            &mut IndexMap::default(),
            &mut V3Packages::default(),
            filename.clone(),
            indexed,
            RepodataRevision::Legacy,
        )
        .unwrap();

        assert!(packages.contains_key(&filename));
    }

    #[test]
    fn legacy_repodata_rejects_v3_matchspecs_after_patching() {
        let mut repodata = RepoData {
            info: None,
            packages: IndexMap::default(),
            conda_packages: IndexMap::default(),
            v3: V3Packages::default(),
            removed: HashSet::default(),
            version: None,
        };
        let mut record = PackageRecord::new(
            PackageName::new_unchecked("demo"),
            Version::from_str("1.0").unwrap(),
            "0".to_string(),
        );
        record.depends = vec!["python[extras=[\"test\"]]".to_string()];
        repodata.packages.insert(
            DistArchiveIdentifier::try_from_filename("demo-1.0-0.tar.bz2").unwrap(),
            record,
        );

        let error = validate_repodata_matchspecs(&mut repodata).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("legacy repodata cannot represent MatchSpec")
        );
    }

    #[test]
    fn latest_assignment_canonicalizes_validated_legacy_index_json() {
        let filename = DistArchiveIdentifier::try_from_filename("demo-1.0-0.tar.bz2").unwrap();
        let index_json = r#"{
                "build": "0",
                "build_number": 0,
                "depends": ["python >=3.10"],
                "name": "demo",
                "version": "1.0"
            }"#;
        let indexed = ParsedPackage::new(b"package", index_json.to_owned(), None)
            .to_indexed_record()
            .unwrap();
        assert_eq!(indexed.repodata_revision, RepodataRevision::Legacy);

        let mut v3 = V3Packages::default();
        insert_package_record_by_revision(
            &mut IndexMap::default(),
            &mut IndexMap::default(),
            &mut v3,
            filename.clone(),
            indexed,
            RepodataRevision::V3,
        )
        .unwrap();

        assert_eq!(
            v3.tar_bz2[&filename.identifier].depends,
            ["python[version=\">=3.10\"]"]
        );
    }

    #[test]
    fn v3_patches_are_canonicalized_before_publication() {
        let identifier = ArchiveIdentifier::from_str("demo-1.0-0").unwrap();
        let mut record = PackageRecord::new(
            PackageName::new_unchecked("demo"),
            Version::from_str("1.0").unwrap(),
            "0".to_string(),
        );
        let patch = serde_json::from_value(serde_json::json!({
            "depends": ["python >=3.10"],
            "constrains": ["python >=3.10"]
        }))
        .unwrap();
        record.apply_patch(&patch);

        let mut repodata = RepoData {
            info: None,
            packages: IndexMap::default(),
            conda_packages: IndexMap::default(),
            v3: V3Packages::default(),
            removed: HashSet::default(),
            version: Some(1),
        };
        repodata.v3.tar_bz2.insert(identifier.clone(), record);
        validate_repodata_matchspecs(&mut repodata).unwrap();

        let record = &repodata.v3.tar_bz2[&identifier];
        assert_eq!(record.depends, ["python[version=\">=3.10\"]"]);
        assert_eq!(record.constrains, ["python[version=\">=3.10\"]"]);
    }

    #[test]
    fn v3_records_from_existing_repodata_are_canonicalized() {
        let filename = DistArchiveIdentifier::try_from_filename("demo-1.0-0.tar.bz2").unwrap();
        let mut record = PackageRecord::new(
            PackageName::new_unchecked("demo"),
            Version::from_str("1.0").unwrap(),
            "0".to_string(),
        );
        record.depends = vec!["python >=3.10".to_string()];
        record.constrains = vec!["python >=3.10".to_string()];
        record
            .extra_depends
            .insert("test".to_string(), vec!["pytest >=8".to_string()]);

        let mut v3 = V3Packages::default();
        insert_package_record_by_revision(
            &mut IndexMap::default(),
            &mut IndexMap::default(),
            &mut v3,
            filename.clone(),
            IndexedPackageRecord {
                record,
                repodata_revision: RepodataRevision::V3,
                matchspecs: None,
                wheel_url: None,
            },
            RepodataRevision::V3,
        )
        .unwrap();

        let record = &v3.tar_bz2[&filename.identifier];
        assert_eq!(record.depends, ["python[version=\">=3.10\"]"]);
        assert_eq!(record.constrains, ["python[version=\">=3.10\"]"]);
        assert_eq!(record.extra_depends["test"], ["pytest[version=\">=8\"]"]);
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
            matchspecs: None,
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
}
