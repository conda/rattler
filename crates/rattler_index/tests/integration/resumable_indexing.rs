//! Tests for the behaviour that makes indexing large channels practical:
//! tolerating broken packages, caching parsed packages on disk, and
//! cooperative cancellation.

use std::{
    fs,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use opendal::Operator;
use rattler_conda_types::{Platform, RepoData, compression_level::CompressionLevel};
use rattler_index::{
    IndexFsConfig, IndexOptions, IndexProcessingOptions, PackageRevisionAssignment,
    PreconditionChecks, index_fs, index_with_options,
};
use rattler_package_streaming::write::write_tar_bz2_package;
use tokio_util::sync::CancellationToken;

use super::etag_memory_backend::{ETagMemoryBuilder, Operation, TestHooks};

/// Writes a minimal but valid `.tar.bz2` package called `<name>-1.0-0` into
/// `subdir_path` and returns its filename.
fn write_package(subdir_path: &Path, name: &str) -> String {
    let build_dir = tempfile::tempdir().unwrap();
    let info_dir = build_dir.path().join("info");
    fs::create_dir_all(&info_dir).unwrap();
    fs::write(
        info_dir.join("index.json"),
        format!(
            r#"{{
                "build": "0",
                "build_number": 0,
                "depends": ["python >=3.10"],
                "name": "{name}",
                "noarch": "generic",
                "subdir": "noarch",
                "timestamp": 1710000000000,
                "version": "1.0"
            }}"#
        ),
    )
    .unwrap();
    let filename = format!("{name}-1.0-0.tar.bz2");
    write_tar_bz2_package(
        File::create(subdir_path.join(&filename)).unwrap(),
        build_dir.path(),
        &[info_dir.join("index.json")],
        CompressionLevel::Default,
        None,
        None,
    )
    .unwrap();
    filename
}

fn fs_config(channel: &Path, processing: IndexProcessingOptions) -> IndexFsConfig {
    IndexFsConfig {
        channel: channel.to_path_buf(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        force: false,
        max_parallel: 4,
        multi_progress: None,
        processing,
    }
}

fn read_repodata(channel: &Path) -> RepoData {
    RepoData::from_path(channel.join("noarch/repodata.json")).unwrap()
}

fn package_names(repodata: &RepoData) -> Vec<String> {
    let mut names = repodata
        .packages
        .keys()
        .chain(repodata.conda_packages.keys())
        .map(rattler_conda_types::package::DistArchiveIdentifier::to_file_name)
        .collect::<Vec<_>>();
    names.sort();
    names
}

/// Validates that packages which cannot be parsed do not abort indexing.
///
/// A corrupt `.conda` used to panic and a corrupt `.tar.bz2` used to abort the
/// whole subdir. Both are now reported in the stats, the valid package is
/// still written to `repodata.json`, and the run itself succeeds.
#[tokio::test]
async fn test_broken_packages_are_skipped_and_reported() {
    let channel = tempfile::tempdir().unwrap();
    let subdir_path = channel.path().join("noarch");
    fs::create_dir_all(&subdir_path).unwrap();
    let valid = write_package(&subdir_path, "valid");
    fs::write(
        subdir_path.join("broken-1.0-0.conda"),
        b"this is not a zip file",
    )
    .unwrap();
    fs::write(
        subdir_path.join("garbage-1.0-0.tar.bz2"),
        b"nor a bz2 tarball",
    )
    .unwrap();

    let stats = index_fs(fs_config(channel.path(), IndexProcessingOptions::default()))
        .await
        .unwrap();

    assert!(stats.has_failures());
    assert!(!stats.cancelled);
    let noarch = &stats.subdirs[&Platform::NoArch];
    assert_eq!(noarch.packages_added, 1);
    assert_eq!(noarch.packages_skipped, 0);
    assert!(noarch.repodata_written);
    let failed = noarch
        .failed_packages
        .iter()
        .map(|failed| failed.filename.as_str())
        .collect::<Vec<_>>();
    assert_eq!(failed, ["broken-1.0-0.conda", "garbage-1.0-0.tar.bz2"]);

    let repodata = read_repodata(channel.path());
    assert_eq!(package_names(&repodata), [valid]);
}

/// Counts the non-empty lines of a cache file.
fn cache_lines(path: &Path) -> Vec<serde_json::Value> {
    BufReader::new(File::open(path).unwrap())
        .lines()
        .map(Result::unwrap)
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(&line).unwrap())
        .collect()
}

/// Validates the on-disk package cache.
///
/// - Parsed and broken packages are both recorded, one line per package.
/// - A second run serves records from the cache. This is made observable by
///   replacing the package contents with garbage while keeping size and
///   modification time: without the cache the package would fail, with the
///   cache its record is reused.
/// - Changing the file (its size) invalidates the entry and the package is
///   parsed again.
/// - Entries for files that disappeared are dropped on compaction.
#[tokio::test]
async fn test_disk_cache_is_reused_and_invalidated() {
    let channel = tempfile::tempdir().unwrap();
    let cache_dir = tempfile::tempdir().unwrap();
    let subdir_path = channel.path().join("noarch");
    fs::create_dir_all(&subdir_path).unwrap();
    let valid = write_package(&subdir_path, "valid");
    fs::write(
        subdir_path.join("broken-1.0-0.conda"),
        b"this is not a zip file",
    )
    .unwrap();

    let processing = || IndexProcessingOptions {
        cache_dir: Some(cache_dir.path().to_path_buf()),
        ..IndexProcessingOptions::default()
    };

    // First run: both packages are parsed, one succeeds.
    let stats = index_fs(fs_config(channel.path(), processing()))
        .await
        .unwrap();
    assert_eq!(stats.subdirs[&Platform::NoArch].packages_added, 1);
    assert_eq!(stats.subdirs[&Platform::NoArch].failed_packages.len(), 1);

    let cache_file = cache_dir.path().join("noarch.jsonl");
    let lines = cache_lines(&cache_file);
    assert_eq!(lines.len(), 2, "one line per package: {lines:?}");
    let by_path = |path: &str| {
        lines
            .iter()
            .find(|line| line["path"] == path)
            .unwrap_or_else(|| panic!("no cache line for {path}"))
    };
    let valid_line = by_path(&format!("noarch/{valid}"));
    assert!(valid_line["package"]["index_json"].is_string());
    assert!(valid_line["package"]["sha256"].is_string());
    assert!(valid_line["error"].is_null());
    let broken_line = by_path("noarch/broken-1.0-0.conda");
    assert!(broken_line["package"].is_null());
    assert!(broken_line["error"].is_string());
    let expected_sha256 = read_repodata(channel.path())
        .packages
        .values()
        .next()
        .unwrap()
        .sha256
        .unwrap();

    // Replace the valid package with garbage of the same size and restore the
    // modification time, so only the cache can still produce its record.
    let package_path = subdir_path.join(&valid);
    let original_len = fs::metadata(&package_path).unwrap().len() as usize;
    let modified = fs::metadata(&package_path).unwrap().modified().unwrap();
    fs::write(&package_path, vec![b'x'; original_len]).unwrap();
    File::options()
        .write(true)
        .open(&package_path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    fs::remove_file(channel.path().join("noarch/repodata.json")).unwrap();

    // Second run with the cache: the record comes from the cache, the broken
    // package is reported from the cache without being parsed again.
    let stats = index_fs(fs_config(channel.path(), processing()))
        .await
        .unwrap();
    let noarch = &stats.subdirs[&Platform::NoArch];
    assert_eq!(noarch.packages_added, 1);
    assert_eq!(noarch.failed_packages.len(), 1);
    assert!(
        noarch.failed_packages[0].error.contains("cached failure"),
        "{}",
        noarch.failed_packages[0].error
    );
    let repodata = read_repodata(channel.path());
    assert_eq!(package_names(&repodata), std::slice::from_ref(&valid));
    assert_eq!(
        repodata.packages.values().next().unwrap().sha256.unwrap(),
        expected_sha256,
        "the cached digest of the original archive is reused"
    );

    // Without the cache the garbage is parsed and fails.
    fs::remove_file(channel.path().join("noarch/repodata.json")).unwrap();
    let stats = index_fs(fs_config(channel.path(), IndexProcessingOptions::default()))
        .await
        .unwrap();
    assert_eq!(stats.subdirs[&Platform::NoArch].packages_added, 0);
    assert_eq!(stats.subdirs[&Platform::NoArch].failed_packages.len(), 2);

    // Changing the size invalidates the cache entry and the package is parsed
    // again, which now fails. Removing the broken package drops its entry.
    fs::write(&package_path, vec![b'x'; original_len + 1]).unwrap();
    fs::remove_file(subdir_path.join("broken-1.0-0.conda")).unwrap();
    fs::remove_file(channel.path().join("noarch/repodata.json")).unwrap();
    let stats = index_fs(fs_config(channel.path(), processing()))
        .await
        .unwrap();
    let noarch = &stats.subdirs[&Platform::NoArch];
    assert_eq!(noarch.packages_added, 0);
    assert_eq!(noarch.failed_packages.len(), 1);
    assert_eq!(noarch.failed_packages[0].filename, valid);
    assert!(!noarch.failed_packages[0].error.contains("cached failure"));

    let lines = cache_lines(&cache_file);
    assert_eq!(
        lines.len(),
        1,
        "compaction keeps one line per existing file"
    );
    assert_eq!(lines[0]["path"], format!("noarch/{valid}"));
    assert!(lines[0]["error"].is_string());
    assert_eq!(lines[0]["size"], (original_len + 1) as u64);
}

/// Sets up a memory backend with three packages whose first package read
/// cancels `token`.
async fn channel_cancelled_on_first_read(token: CancellationToken) -> (Operator, Vec<String>) {
    let reads = Arc::new(AtomicUsize::new(0));
    let hooks = TestHooks {
        on_operation: Arc::new(move |path, operation| {
            let token = token.clone();
            let reads = reads.clone();
            let is_package_read = operation == Operation::BeforeRead && path.ends_with(".tar.bz2");
            Box::pin(async move {
                if is_package_read && reads.fetch_add(1, Ordering::SeqCst) == 0 {
                    token.cancel();
                }
            })
        }),
    };
    let op = Operator::new(ETagMemoryBuilder::default().with_test_hooks(hooks))
        .unwrap()
        .finish();

    let source = tempfile::tempdir().unwrap();
    let mut filenames = Vec::new();
    for name in ["a", "b", "c"] {
        let filename = write_package(source.path(), name);
        let bytes = fs::read(source.path().join(&filename)).unwrap();
        op.write(&format!("noarch/{filename}"), bytes)
            .await
            .unwrap();
        filenames.push(filename);
    }
    (op, filenames)
}

fn cancellable_options(token: CancellationToken, publish_partial: bool) -> IndexOptions {
    IndexOptions {
        target_platform: Some(Platform::NoArch),
        max_parallel: 1,
        precondition_checks: PreconditionChecks::Enabled,
        processing: IndexProcessingOptions {
            cancellation_token: Some(token),
            publish_partial,
            ..IndexProcessingOptions::default()
        },
        ..IndexOptions::default()
    }
}

/// Validates cooperative cancellation without partial publishing.
///
/// With one package in flight at a time, cancelling during the first read
/// lets that package finish, starts no other, and leaves the repodata
/// untouched.
#[tokio::test]
async fn test_cancellation_finishes_in_flight_package_and_skips_the_rest() {
    let token = CancellationToken::new();
    let (op, _) = channel_cancelled_on_first_read(token.clone()).await;

    let stats = index_with_options(op.clone(), cancellable_options(token.clone(), false))
        .await
        .unwrap();

    assert!(token.is_cancelled());
    assert!(stats.cancelled);
    let noarch = &stats.subdirs[&Platform::NoArch];
    assert!(noarch.cancelled);
    assert_eq!(noarch.packages_added, 1);
    assert_eq!(noarch.packages_skipped, 2);
    assert!(noarch.failed_packages.is_empty());
    assert!(!noarch.repodata_written);
    assert!(!op.exists("noarch/repodata.json").await.unwrap());
}

/// Validates cooperative cancellation with partial publishing: the repodata is
/// written with the packages indexed so far, and a later uncancelled run adds
/// the remaining ones.
#[tokio::test]
async fn test_cancellation_can_publish_partial_repodata_and_resume() {
    let token = CancellationToken::new();
    let (op, filenames) = channel_cancelled_on_first_read(token.clone()).await;

    let stats = index_with_options(op.clone(), cancellable_options(token, true))
        .await
        .unwrap();
    let noarch = &stats.subdirs[&Platform::NoArch];
    assert!(stats.cancelled);
    assert_eq!(noarch.packages_added, 1);
    assert_eq!(noarch.packages_skipped, 2);
    assert!(noarch.repodata_written);

    let repodata: RepoData =
        serde_json::from_slice(&op.read("noarch/repodata.json").await.unwrap().to_bytes()).unwrap();
    // Packages are processed in filename order, so the first one landed.
    assert_eq!(package_names(&repodata), [filenames[0].clone()]);

    // Resuming picks up the two remaining packages.
    let stats = index_with_options(
        op.clone(),
        cancellable_options(CancellationToken::new(), false),
    )
    .await
    .unwrap();
    assert!(!stats.cancelled);
    assert_eq!(stats.subdirs[&Platform::NoArch].packages_added, 2);
    let repodata: RepoData =
        serde_json::from_slice(&op.read("noarch/repodata.json").await.unwrap().to_bytes()).unwrap();
    assert_eq!(package_names(&repodata), filenames);
}

/// Validates that a byte budget smaller than a single package still lets the
/// package through (clamped to the budget) instead of deadlocking.
#[tokio::test]
async fn test_byte_budget_smaller_than_a_package_does_not_deadlock() {
    let channel = tempfile::tempdir().unwrap();
    let subdir_path = channel.path().join("noarch");
    fs::create_dir_all(&subdir_path).unwrap();
    let mut filenames = vec![
        write_package(&subdir_path, "a"),
        write_package(&subdir_path, "b"),
        write_package(&subdir_path, "c"),
    ];
    filenames.sort();

    let stats = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        index_fs(fs_config(
            channel.path(),
            IndexProcessingOptions {
                max_in_flight_bytes: Some(1),
                ..IndexProcessingOptions::default()
            },
        )),
    )
    .await
    .expect("indexing must not deadlock")
    .unwrap();

    assert!(!stats.has_failures());
    assert_eq!(stats.subdirs[&Platform::NoArch].packages_added, 3);
    assert_eq!(package_names(&read_repodata(channel.path())), filenames);
}
