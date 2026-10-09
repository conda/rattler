//! Tests for the behaviour that makes indexing large channels practical:
//! tolerating broken packages, caching parsed packages in the channel, and
//! cooperative cancellation.

use std::{
    fs,
    fs::File,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use opendal::Operator;
use rattler_conda_types::{RepoData, Subdir, compression_level::CompressionLevel};
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
        target_platform: Some(Subdir::NoArch),
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
    let noarch = &stats.subdirs[&Subdir::NoArch];
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

/// Returns the cache objects of a subdir as `(object name, body)`.
fn cache_objects(subdir_path: &Path) -> Vec<(String, serde_json::Value)> {
    let cache_dir = subdir_path.join(".cache");
    if !cache_dir.exists() {
        return Vec::new();
    }
    let mut objects = fs::read_dir(cache_dir)
        .unwrap()
        .map(Result::unwrap)
        .map(|entry| {
            let body = serde_json::from_slice(&fs::read(entry.path()).unwrap()).unwrap();
            (entry.file_name().to_string_lossy().into_owned(), body)
        })
        .collect::<Vec<_>>();
    objects.sort_by(|a, b| a.0.cmp(&b.0));
    objects
}

fn with_cache() -> IndexProcessingOptions {
    IndexProcessingOptions {
        cache: true,
        ..IndexProcessingOptions::default()
    }
}

/// Validates the package cache stored in the channel.
///
/// - Parsed and broken packages are both recorded, one object per package.
/// - A second run serves records from the cache. This is made observable by
///   replacing the package contents with garbage while keeping size and
///   modification time: without the cache the package would fail, with the
///   cache its record is reused.
/// - Changing the file (its size) invalidates the entry and the package is
///   parsed again.
/// - Objects for files that disappeared or were replaced are pruned.
#[tokio::test]
async fn test_channel_cache_is_reused_and_invalidated() {
    let channel = tempfile::tempdir().unwrap();
    let subdir_path = channel.path().join("noarch");
    fs::create_dir_all(&subdir_path).unwrap();
    let valid = write_package(&subdir_path, "valid");
    fs::write(
        subdir_path.join("broken-1.0-0.conda"),
        b"this is not a zip file",
    )
    .unwrap();

    // First run: both packages are parsed, one succeeds.
    let stats = index_fs(fs_config(channel.path(), with_cache()))
        .await
        .unwrap();
    assert_eq!(stats.subdirs[&Subdir::NoArch].packages_added, 1);
    assert_eq!(stats.subdirs[&Subdir::NoArch].failed_packages.len(), 1);

    let objects = cache_objects(&subdir_path);
    assert_eq!(objects.len(), 2, "one object per package: {objects:?}");
    let by_prefix = |prefix: &str| {
        objects
            .iter()
            .find(|(name, _)| name.starts_with(prefix))
            .unwrap_or_else(|| panic!("no cache object for {prefix}"))
    };
    let (valid_name, valid_body) = by_prefix(&format!("{valid}."));
    assert!(valid_name.ends_with(".json"));
    assert_eq!(valid_body["version"], 1);
    assert_eq!(valid_body["package"]["index_json"]["name"], "valid");
    assert!(valid_body["package"]["sha256"].is_string());
    assert!(valid_body["error"].is_null());
    let (_, broken_body) = by_prefix("broken-1.0-0.conda.");
    assert!(broken_body["package"].is_null());
    assert!(broken_body["error"].is_string());
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
    let stats = index_fs(fs_config(channel.path(), with_cache()))
        .await
        .unwrap();
    let noarch = &stats.subdirs[&Subdir::NoArch];
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
    assert_eq!(stats.subdirs[&Subdir::NoArch].packages_added, 0);
    assert_eq!(stats.subdirs[&Subdir::NoArch].failed_packages.len(), 2);
    assert_eq!(
        cache_objects(&subdir_path).len(),
        2,
        "no cache: nothing changes"
    );

    // Changing the size invalidates the cache entry and the package is parsed
    // again, which now fails. Removing the broken package drops its object,
    // and the superseded object of the changed package is pruned too.
    fs::write(&package_path, vec![b'x'; original_len + 1]).unwrap();
    fs::remove_file(subdir_path.join("broken-1.0-0.conda")).unwrap();
    fs::remove_file(channel.path().join("noarch/repodata.json")).unwrap();
    let stats = index_fs(fs_config(channel.path(), with_cache()))
        .await
        .unwrap();
    let noarch = &stats.subdirs[&Subdir::NoArch];
    assert_eq!(noarch.packages_added, 0);
    assert_eq!(noarch.failed_packages.len(), 1);
    assert_eq!(noarch.failed_packages[0].filename, valid);
    assert!(!noarch.failed_packages[0].error.contains("cached failure"));

    let objects = cache_objects(&subdir_path);
    assert_eq!(
        objects.len(),
        1,
        "one object per existing file version: {objects:?}"
    );
    assert!(objects[0].0.starts_with(&format!("{valid}.")));
    assert!(objects[0].1["error"].is_string());
    assert_eq!(objects[0].1["size"], (original_len + 1) as u64);
}

/// Validates that a cache object written by another format version is ignored
/// and the package is parsed again.
#[tokio::test]
async fn test_channel_cache_ignores_other_format_version() {
    let channel = tempfile::tempdir().unwrap();
    let subdir_path = channel.path().join("noarch");
    fs::create_dir_all(&subdir_path).unwrap();
    let valid = write_package(&subdir_path, "valid");

    // Learn the object name from a real run, then replace the body.
    index_fs(fs_config(channel.path(), with_cache()))
        .await
        .unwrap();
    let objects = cache_objects(&subdir_path);
    assert_eq!(objects.len(), 1);
    let object_path = subdir_path.join(".cache").join(&objects[0].0);
    fs::write(
        &object_path,
        serde_json::to_vec(&serde_json::json!({ "version": 99, "error": "from the future" }))
            .unwrap(),
    )
    .unwrap();
    fs::remove_file(channel.path().join("noarch/repodata.json")).unwrap();

    let stats = index_fs(fs_config(channel.path(), with_cache()))
        .await
        .unwrap();
    assert!(!stats.has_failures());
    assert_eq!(stats.subdirs[&Subdir::NoArch].packages_added, 1);
    assert_eq!(package_names(&read_repodata(channel.path())), [valid]);
}

/// Validates that two indexers sharing one channel cache do not interfere:
/// both finish, every package ends up in the repodata, and there is exactly
/// one cache object per package.
#[tokio::test]
async fn test_channel_cache_is_shared_between_concurrent_indexers() {
    let op = Operator::new(ETagMemoryBuilder::default())
        .unwrap()
        .finish();
    let source = tempfile::tempdir().unwrap();
    let mut filenames = Vec::new();
    for name in ["a", "b", "c", "d"] {
        let filename = write_package(source.path(), name);
        let bytes = fs::read(source.path().join(&filename)).unwrap();
        op.write(&format!("noarch/{filename}"), bytes)
            .await
            .unwrap();
        filenames.push(filename);
    }
    let options = || IndexOptions {
        target_platform: Some(Subdir::NoArch),
        max_parallel: 2,
        precondition_checks: PreconditionChecks::Enabled,
        processing: with_cache(),
        ..IndexOptions::default()
    };

    let (first, second) = tokio::join!(
        index_with_options(op.clone(), options()),
        index_with_options(op.clone(), options()),
    );
    let (first, second) = (first.unwrap(), second.unwrap());
    assert!(!first.has_failures() && !second.has_failures());

    let repodata: RepoData =
        serde_json::from_slice(&op.read("noarch/repodata.json").await.unwrap().to_bytes()).unwrap();
    assert_eq!(package_names(&repodata), filenames);

    let mut cached = op
        .list_with("noarch/.cache/")
        .await
        .unwrap()
        .into_iter()
        .filter(|entry| entry.metadata().mode().is_file())
        .map(|entry| entry.name().to_owned())
        .collect::<Vec<_>>();
    cached.sort();
    assert_eq!(cached.len(), filenames.len(), "{cached:?}");
    for (object, filename) in cached.iter().zip(&filenames) {
        assert!(object.starts_with(&format!("{filename}.")), "{object}");
    }
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

fn cancellable_options(token: CancellationToken) -> IndexOptions {
    IndexOptions {
        target_platform: Some(Subdir::NoArch),
        max_parallel: 1,
        precondition_checks: PreconditionChecks::Enabled,
        processing: IndexProcessingOptions {
            cancellation_token: Some(token),
            ..IndexProcessingOptions::default()
        },
        ..IndexOptions::default()
    }
}

/// Validates cooperative cancellation.
///
/// With one package in flight at a time, cancelling during the first read
/// lets that package finish, starts no other, and leaves the repodata
/// untouched. A later uncancelled run indexes all packages.
#[tokio::test]
async fn test_cancellation_finishes_in_flight_package_and_skips_the_rest() {
    let token = CancellationToken::new();
    let (op, filenames) = channel_cancelled_on_first_read(token.clone()).await;

    let stats = index_with_options(op.clone(), cancellable_options(token.clone()))
        .await
        .unwrap();

    assert!(token.is_cancelled());
    assert!(stats.cancelled);
    let noarch = &stats.subdirs[&Subdir::NoArch];
    assert!(noarch.cancelled);
    assert_eq!(noarch.packages_added, 1);
    assert_eq!(noarch.packages_skipped, 2);
    assert!(noarch.failed_packages.is_empty());
    assert!(!noarch.repodata_written);
    assert!(!op.exists("noarch/repodata.json").await.unwrap());

    // Resuming indexes everything.
    let stats = index_with_options(op.clone(), cancellable_options(CancellationToken::new()))
        .await
        .unwrap();
    assert!(!stats.cancelled);
    assert_eq!(stats.subdirs[&Subdir::NoArch].packages_added, 3);
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
    assert_eq!(stats.subdirs[&Subdir::NoArch].packages_added, 3);
    assert_eq!(package_names(&read_repodata(channel.path())), filenames);
}
