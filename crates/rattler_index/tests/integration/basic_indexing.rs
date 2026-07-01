use std::{
    fs,
    fs::File,
    path::{Path, PathBuf},
};

use rattler_conda_types::{
    ChannelRelations, Platform, ShardedRepodata, compression_level::CompressionLevel,
};
use rattler_index::{
    BackfillIndexedTimestamps, ChannelMetadata, IndexFsConfig, PackageRevisionAssignment,
    RepodataRevision, RepodataRevisionInfo, index_fs, index_fs_with_channel_metadata,
};
use rattler_package_streaming::write::write_tar_bz2_package;
use serde_json::Value;

fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
}

/// Validates that indexing creates correct repodata.json for .conda and .tar.bz2 packages.
///
/// This test downloads sample packages, indexes them, and verifies:
/// - The subdir is correctly set
/// - Both .tar.bz2 and .conda packages are indexed
/// - Package records match expected values
#[tokio::test]
async fn test_index() {
    let test_start_millis = current_unix_millis();
    let temp_dir = tempfile::tempdir().unwrap();
    let subdir_path = Path::new("win-64");
    let conda_file_path = tokio::task::spawn_blocking(|| {
        tools::download_and_cache_file(
            "https://conda.anaconda.org/conda-forge/win-64/conda-22.11.1-py38haa244fe_1.conda"
                .parse()
                .unwrap(),
            "a8a44c5ff2b2f423546d49721ba2e3e632233c74a813c944adf8e5742834930e",
        )
    })
    .await
    .unwrap()
    .unwrap();
    let index_json_path = Path::new("conda-22.11.1-py38haa244fe_1-index.json");
    let tar_bz2_file_path = tokio::task::spawn_blocking(|| {
        tools::download_and_cache_file(
            "https://conda.anaconda.org/conda-forge/win-64/conda-22.9.0-py38haa244fe_2.tar.bz2"
                .parse()
                .unwrap(),
            "3c2c2e8e81bde5fb1ac4b014f51a62411feff004580c708c97a0ec2b7058cdc4",
        )
    })
    .await
    .unwrap()
    .unwrap();

    fs::create_dir(temp_dir.path().join(subdir_path)).unwrap();
    fs::copy(
        &conda_file_path,
        temp_dir
            .path()
            .join(subdir_path)
            .join(conda_file_path.file_name().unwrap()),
    )
    .unwrap();
    fs::copy(
        &tar_bz2_file_path,
        temp_dir
            .path()
            .join(subdir_path)
            .join(tar_bz2_file_path.file_name().unwrap()),
    )
    .unwrap();

    let res = index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::Win64),
        repodata_patch: None,
        write_zst: true,
        write_shards: true,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        backfill_indexed_timestamps: BackfillIndexedTimestamps::default(),
        force: true,
        max_parallel: 32,
        multi_progress: None,
    })
    .await;
    if let Err(e) = &res {
        eprintln!("Error: {e:?}");
    }
    assert!(res.is_ok());

    let repodata_path = temp_dir.path().join(subdir_path).join("repodata.json");
    let repodata_json: Value = serde_json::from_reader(File::open(repodata_path).unwrap()).unwrap();

    let expected_repodata_entry: Value =
        serde_json::from_reader(File::open(test_data_dir().join(index_json_path)).unwrap())
            .unwrap();

    assert_eq!(
        repodata_json
            .get("info")
            .unwrap()
            .get("subdir")
            .unwrap()
            .as_str(),
        Some("win-64")
    );
    assert!(
        repodata_json
            .get("packages")
            .unwrap()
            .get("conda-22.9.0-py38haa244fe_2.tar.bz2")
            .is_some()
    );
    let mut actual_entry = repodata_json
        .get("packages.conda")
        .unwrap()
        .get("conda-22.11.1-py38haa244fe_1.conda")
        .unwrap()
        .clone();
    // `indexed_timestamp` is assigned by the indexer and not part of the
    // build-tool fixture; verify it separately and strip it before comparing.
    let indexed_timestamp = actual_entry
        .as_object_mut()
        .unwrap()
        .remove("indexed_timestamp")
        .unwrap();
    assert!(indexed_timestamp.as_i64().unwrap() >= test_start_millis);
    assert_eq!(actual_entry, expected_repodata_entry);
}

fn current_unix_millis() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// Validates that indexing an empty directory creates a noarch subdir with repodata files.
///
/// This test verifies that:
/// - A noarch directory is automatically created
/// - repodata.json is created even with no packages
/// - Compressed and sharded variants are created when requested
#[tokio::test]
async fn test_index_empty_directory_creates_noarch_repodata() {
    let temp_dir = tempfile::tempdir().unwrap();
    let noarch_path = temp_dir.path().join("noarch");
    let repodata_path = noarch_path.join("repodata.json");
    let repodata_zst_path = noarch_path.join("repodata.json");
    let repodata_msgpack_path = noarch_path.join("repodata_shards.msgpack.zst");

    let res = index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: None,
        repodata_patch: None,
        write_zst: true,
        write_shards: true,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        backfill_indexed_timestamps: BackfillIndexedTimestamps::default(),
        force: true,
        max_parallel: 100,
        multi_progress: None,
    })
    .await;

    if let Err(e) = &res {
        eprintln!("Error in empty directory test: {e:?}");
    }
    assert!(res.is_ok());
    assert!(noarch_path.is_dir());
    assert_eq!(fs::read_dir(&noarch_path).unwrap().count(), 3);
    assert!(repodata_path.is_file());
    assert!(repodata_zst_path.is_file());
    assert!(repodata_msgpack_path.is_file());
}

/// Validates that reindexing removes stale package entries from repodata when
/// the package file is deleted from disk.
#[tokio::test]
async fn test_reindex_removes_deleted_conda_package() {
    let temp_dir = tempfile::tempdir().unwrap();
    let subdir_path = temp_dir.path().join("noarch");
    let package_name = "empty-0.1.0-h4616a5c_0.conda";
    let source_package = test_data_dir().join("packages").join(package_name);
    let target_package = subdir_path.join(package_name);

    fs::create_dir(&subdir_path).unwrap();
    fs::copy(source_package, &target_package).unwrap();

    index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        backfill_indexed_timestamps: BackfillIndexedTimestamps::default(),
        force: false,
        max_parallel: 1,
        multi_progress: None,
    })
    .await
    .unwrap();

    let repodata_path = subdir_path.join("repodata.json");
    let repodata_json: Value =
        serde_json::from_reader(File::open(&repodata_path).unwrap()).unwrap();
    assert!(
        repodata_json
            .get("packages.conda")
            .unwrap()
            .get(package_name)
            .is_some()
    );

    fs::remove_file(target_package).unwrap();

    index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        backfill_indexed_timestamps: BackfillIndexedTimestamps::default(),
        force: false,
        max_parallel: 1,
        multi_progress: None,
    })
    .await
    .unwrap();

    let repodata_json: Value = serde_json::from_reader(File::open(repodata_path).unwrap()).unwrap();
    assert!(
        repodata_json
            .get("packages.conda")
            .unwrap()
            .get(package_name)
            .is_none()
    );
}

#[tokio::test]
async fn test_index_latest_repodata_revision() {
    let temp_dir = tempfile::tempdir().unwrap();
    let subdir_path = temp_dir.path().join("noarch");
    let package_name = "empty-0.1.0-h4616a5c_0.conda";
    let source_package = test_data_dir().join("packages").join(package_name);
    let target_package = subdir_path.join(package_name);

    fs::create_dir(&subdir_path).unwrap();
    fs::copy(source_package, &target_package).unwrap();

    index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: true,
        write_shards: true,
        repodata_revisions: vec![RepodataRevisionInfo {
            revision: RepodataRevision::V3,
            n_packages: None,
            oldest: None,
            newest: None,
        }],
        package_revision_assignment: PackageRevisionAssignment::Latest,
        backfill_indexed_timestamps: BackfillIndexedTimestamps::default(),
        force: true,
        max_parallel: 1,
        multi_progress: None,
    })
    .await
    .unwrap();

    let repodata_path = subdir_path.join("repodata.json");
    let repodata_json: Value =
        serde_json::from_reader(File::open(&repodata_path).unwrap()).unwrap();
    assert!(
        repodata_json
            .get("packages.conda")
            .unwrap()
            .as_object()
            .unwrap()
            .is_empty()
    );
    assert!(
        repodata_json
            .pointer("/v3/conda/empty-0.1.0-h4616a5c_0")
            .is_some()
    );
    let revision = &repodata_json["info"]["repodata_revisions"]["v3"];
    assert_eq!(revision["n_packages"], 1);

    let shard_index_bytes = fs::read(subdir_path.join("repodata_shards.msgpack.zst")).unwrap();
    let shard_index_bytes = zstd::decode_all(shard_index_bytes.as_slice()).unwrap();
    let shard_index: ShardedRepodata = rmp_serde::from_slice(&shard_index_bytes).unwrap();
    assert_eq!(shard_index.info.repodata_revisions.len(), 1);
    assert_eq!(
        shard_index.info.repodata_revisions[&RepodataRevision::V3].n_packages,
        Some(1)
    );
}

#[tokio::test]
async fn test_index_repodata_revision_from_index_json() {
    let temp_dir = tempfile::tempdir().unwrap();
    let subdir_path = temp_dir.path().join("noarch");
    let package_name = "revision-demo-1.0.0-h123_0.tar.bz2";
    let package_build_dir = temp_dir.path().join("package-build");
    let package_info_dir = package_build_dir.join("info");

    fs::create_dir(&subdir_path).unwrap();
    fs::create_dir(&package_build_dir).unwrap();
    fs::create_dir(&package_info_dir).unwrap();
    fs::write(
        package_info_dir.join("index.json"),
        r#"{
            "build": "h123_0",
            "build_number": 0,
            "extra_depends": {
                "docs": ["sphinx"]
            },
            "name": "revision-demo",
            "noarch": "generic",
            "subdir": "noarch",
            "timestamp": 1710000000000,
            "version": "1.0.0"
        }"#,
    )
    .unwrap();

    let target_package = subdir_path.join(package_name);
    let writer = File::create(&target_package).unwrap();
    write_tar_bz2_package(
        writer,
        &package_build_dir,
        &[package_info_dir.join("index.json")],
        CompressionLevel::Default,
        None,
        None,
    )
    .unwrap();

    fs::copy(
        test_data_dir()
            .join("packages")
            .join("empty-0.1.0-h4616a5c_0.conda"),
        subdir_path.join("empty-0.1.0-h4616a5c_0.conda"),
    )
    .unwrap();

    index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        repodata_revisions: vec![RepodataRevisionInfo {
            revision: RepodataRevision::V3,
            n_packages: None,
            oldest: None,
            newest: None,
        }],
        package_revision_assignment: PackageRevisionAssignment::FromIndexJson,
        backfill_indexed_timestamps: BackfillIndexedTimestamps::default(),
        force: true,
        max_parallel: 1,
        multi_progress: None,
    })
    .await
    .unwrap();

    let repodata_path = subdir_path.join("repodata.json");
    let repodata_json: Value =
        serde_json::from_reader(File::open(&repodata_path).unwrap()).unwrap();
    assert!(
        repodata_json
            .pointer("/packages.conda/empty-0.1.0-h4616a5c_0.conda")
            .is_some()
    );
    assert!(
        repodata_json
            .pointer("/packages/revision-demo-1.0.0-h123_0.tar.bz2")
            .is_none()
    );
    assert!(
        repodata_json
            .pointer("/v3/tar.bz2/revision-demo-1.0.0-h123_0")
            .is_some()
    );
    let revision = &repodata_json["info"]["repodata_revisions"]["v3"];
    assert_eq!(revision["n_packages"], 1);
    assert_eq!(revision["oldest"], 1710000000000i64);
    assert_eq!(revision["newest"], 1710000000000i64);
}

#[tokio::test]
async fn test_index_writes_channel_metadata() {
    let temp_dir = tempfile::tempdir().unwrap();
    let subdir_path = temp_dir.path().join("noarch");
    let channel_metadata = ChannelMetadata {
        base_url: Some("../packages/".to_string()),
        channel_relations: Some(ChannelRelations {
            base: Some("../conda-forge".to_string()),
            overrides: Some("../fallback".to_string()),
        }),
    };

    index_fs_with_channel_metadata(
        IndexFsConfig {
            channel: temp_dir.path().into(),
            target_platform: Some(Platform::NoArch),
            repodata_patch: None,
            write_zst: true,
            write_shards: true,
            repodata_revisions: vec![RepodataRevisionInfo {
                revision: RepodataRevision::V3,
                n_packages: None,
                oldest: None,
                newest: None,
            }],
            package_revision_assignment: PackageRevisionAssignment::Latest,
            backfill_indexed_timestamps: BackfillIndexedTimestamps::default(),
            force: true,
            max_parallel: 1,
            multi_progress: None,
        },
        channel_metadata,
    )
    .await
    .unwrap();

    let repodata_path = subdir_path.join("repodata.json");
    let repodata_json: Value =
        serde_json::from_reader(File::open(&repodata_path).unwrap()).unwrap();
    assert_eq!(repodata_json["info"]["base_url"], "../packages/");
    assert_eq!(
        repodata_json["info"]["channel_relations"]["base"],
        "../conda-forge"
    );
    assert_eq!(
        repodata_json["info"]["channel_relations"]["overrides"],
        "../fallback"
    );
    assert_eq!(
        repodata_json["info"]["repodata_revisions"]["v3"]["n_packages"],
        0
    );

    let shard_index_bytes = fs::read(subdir_path.join("repodata_shards.msgpack.zst")).unwrap();
    let shard_index_bytes = zstd::decode_all(shard_index_bytes.as_slice()).unwrap();
    let shard_index: ShardedRepodata = rmp_serde::from_slice(&shard_index_bytes).unwrap();
    assert_eq!(shard_index.info.base_url, "../packages/");
    assert_eq!(
        shard_index
            .info
            .channel_relations
            .as_ref()
            .unwrap()
            .base
            .as_deref(),
        Some("../conda-forge")
    );
    assert_eq!(
        shard_index
            .info
            .channel_relations
            .as_ref()
            .unwrap()
            .overrides
            .as_deref(),
        Some("../fallback")
    );
    assert_eq!(
        shard_index.info.repodata_revisions[&RepodataRevision::V3].n_packages,
        Some(0)
    );
}

fn empty_package_index_config(channel: &Path, force: bool) -> IndexFsConfig {
    IndexFsConfig {
        channel: channel.into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        backfill_indexed_timestamps: BackfillIndexedTimestamps::default(),
        force,
        max_parallel: 1,
        multi_progress: None,
    }
}

fn read_indexed_timestamp(repodata_path: &Path, package_name: &str) -> Option<i64> {
    let repodata_json: Value = serde_json::from_reader(File::open(repodata_path).unwrap()).unwrap();
    repodata_json
        .get("packages.conda")
        .and_then(|packages| packages.get(package_name))
        .or_else(|| {
            repodata_json
                .get("packages")
                .and_then(|packages| packages.get(package_name))
        })
        .unwrap()
        .get("indexed_timestamp")
        .map(|value| value.as_i64().unwrap())
}

/// Validates that `indexed_timestamp` is assigned to newly indexed packages
/// and preserved across re-indexing runs, including runs with `force`.
#[tokio::test]
async fn test_indexed_timestamp_assignment_and_preservation() {
    let test_start_millis = current_unix_millis();
    let temp_dir = tempfile::tempdir().unwrap();
    let subdir_path = temp_dir.path().join("noarch");
    let package_name = "empty-0.1.0-h4616a5c_0.conda";

    fs::create_dir(&subdir_path).unwrap();
    fs::copy(
        test_data_dir().join("packages").join(package_name),
        subdir_path.join(package_name),
    )
    .unwrap();

    index_fs(empty_package_index_config(temp_dir.path(), false))
        .await
        .unwrap();

    let repodata_path = subdir_path.join("repodata.json");
    let assigned = read_indexed_timestamp(&repodata_path, package_name).unwrap();
    assert!(assigned >= test_start_millis);
    assert!(assigned <= current_unix_millis());

    // Re-indexing must not recompute the value.
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    index_fs(empty_package_index_config(temp_dir.path(), false))
        .await
        .unwrap();
    assert_eq!(
        read_indexed_timestamp(&repodata_path, package_name).unwrap(),
        assigned
    );

    // Even a force re-index (which rebuilds records from the package
    // archives) must carry the value over.
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    index_fs(empty_package_index_config(temp_dir.path(), true))
        .await
        .unwrap();
    assert_eq!(
        read_indexed_timestamp(&repodata_path, package_name).unwrap(),
        assigned
    );
}

/// Writes a channel containing two packages and a hand-written repodata.json
/// in which one record lacks `indexed_timestamp` (with the given build
/// `timestamp`) and the other has a pre-assigned value.
fn write_backfill_channel(temp_dir: &Path) -> PathBuf {
    let subdir_path = temp_dir.join("noarch");
    let package_name = "empty-0.1.0-h4616a5c_0.conda";

    fs::create_dir(&subdir_path).unwrap();
    fs::copy(
        test_data_dir().join("packages").join(package_name),
        subdir_path.join(package_name),
    )
    .unwrap();

    // A second package so that one record can carry a pre-assigned value.
    let package_build_dir = temp_dir.join("package-build");
    let package_info_dir = package_build_dir.join("info");
    fs::create_dir(&package_build_dir).unwrap();
    fs::create_dir(&package_info_dir).unwrap();
    fs::write(
        package_info_dir.join("index.json"),
        r#"{
            "build": "h123_0",
            "build_number": 0,
            "name": "preset-demo",
            "noarch": "generic",
            "subdir": "noarch",
            "timestamp": 1710000000000,
            "version": "1.0.0"
        }"#,
    )
    .unwrap();
    let writer = File::create(subdir_path.join("preset-demo-1.0.0-h123_0.tar.bz2")).unwrap();
    write_tar_bz2_package(
        writer,
        &package_build_dir,
        &[package_info_dir.join("index.json")],
        CompressionLevel::Default,
        None,
        None,
    )
    .unwrap();

    fs::write(
        subdir_path.join("repodata.json"),
        r#"{
            "info": {"subdir": "noarch"},
            "packages": {
                "preset-demo-1.0.0-h123_0.tar.bz2": {
                    "build": "h123_0",
                    "build_number": 0,
                    "indexed_timestamp": 1650000000000,
                    "name": "preset-demo",
                    "noarch": "generic",
                    "subdir": "noarch",
                    "timestamp": 1710000000000,
                    "version": "1.0.0"
                }
            },
            "packages.conda": {
                "empty-0.1.0-h4616a5c_0.conda": {
                    "build": "h4616a5c_0",
                    "build_number": 0,
                    "name": "empty",
                    "noarch": "generic",
                    "subdir": "noarch",
                    "timestamp": 1710000000000,
                    "version": "0.1.0"
                }
            }
        }"#,
    )
    .unwrap();

    subdir_path.join("repodata.json")
}

/// Validates the backfill modes for records in existing repodata that lack
/// `indexed_timestamp`. Records with a pre-assigned value are never touched.
#[tokio::test]
async fn test_indexed_timestamp_backfill_modes() {
    for (mode, expected) in [
        (
            BackfillIndexedTimestamps::FromCondaPackageTimestamp,
            Some(Some(1_710_000_000_000)),
        ),
        (BackfillIndexedTimestamps::Now, Some(None)),
        (BackfillIndexedTimestamps::Off, None),
    ] {
        let test_start_millis = current_unix_millis();
        let temp_dir = tempfile::tempdir().unwrap();
        let repodata_path = write_backfill_channel(temp_dir.path());

        let mut config = empty_package_index_config(temp_dir.path(), false);
        config.backfill_indexed_timestamps = mode;
        index_fs(config).await.unwrap();

        let backfilled = read_indexed_timestamp(&repodata_path, "empty-0.1.0-h4616a5c_0.conda");
        match expected {
            // Seeded from the record's build timestamp.
            Some(Some(timestamp)) => assert_eq!(backfilled, Some(timestamp), "mode {mode}"),
            // Seeded with the indexing time.
            Some(None) => {
                let backfilled = backfilled.unwrap();
                assert!(backfilled >= test_start_millis, "mode {mode}");
                assert!(backfilled <= current_unix_millis(), "mode {mode}");
            }
            // Left untouched.
            None => assert_eq!(backfilled, None, "mode {mode}"),
        }

        // The pre-assigned value is preserved in all modes.
        assert_eq!(
            read_indexed_timestamp(&repodata_path, "preset-demo-1.0.0-h123_0.tar.bz2"),
            Some(1_650_000_000_000),
            "mode {mode}"
        );
    }
}

/// Validates that a package with a build timestamp in the future is rejected
/// and the subdir's repodata is not written.
#[tokio::test]
async fn test_index_rejects_future_build_timestamp() {
    let temp_dir = tempfile::tempdir().unwrap();
    let subdir_path = temp_dir.path().join("noarch");
    let package_name = "future-demo-1.0.0-h123_0.tar.bz2";
    let package_build_dir = temp_dir.path().join("package-build");
    let package_info_dir = package_build_dir.join("info");

    fs::create_dir(&subdir_path).unwrap();
    fs::create_dir(&package_build_dir).unwrap();
    fs::create_dir(&package_info_dir).unwrap();
    // Year 2100.
    fs::write(
        package_info_dir.join("index.json"),
        r#"{
            "build": "h123_0",
            "build_number": 0,
            "name": "future-demo",
            "noarch": "generic",
            "subdir": "noarch",
            "timestamp": 4102444800000,
            "version": "1.0.0"
        }"#,
    )
    .unwrap();
    let writer = File::create(subdir_path.join(package_name)).unwrap();
    write_tar_bz2_package(
        writer,
        &package_build_dir,
        &[package_info_dir.join("index.json")],
        CompressionLevel::Default,
        None,
        None,
    )
    .unwrap();

    let err = index_fs(empty_package_index_config(temp_dir.path(), false))
        .await
        .unwrap_err();
    assert!(err.to_string().contains(package_name), "error: {err}");
    assert!(
        err.to_string().contains("refusing to index"),
        "error: {err}"
    );
    assert!(!subdir_path.join("repodata.json").exists());
}

/// Validates that `indexed_timestamp` also ends up in the sharded repodata.
#[tokio::test]
async fn test_indexed_timestamp_in_shards() {
    let test_start_millis = current_unix_millis();
    let temp_dir = tempfile::tempdir().unwrap();
    let subdir_path = temp_dir.path().join("noarch");
    let package_name = "empty-0.1.0-h4616a5c_0.conda";

    fs::create_dir(&subdir_path).unwrap();
    fs::copy(
        test_data_dir().join("packages").join(package_name),
        subdir_path.join(package_name),
    )
    .unwrap();

    let mut config = empty_package_index_config(temp_dir.path(), false);
    config.write_shards = true;
    index_fs(config).await.unwrap();

    let shard_index_bytes = fs::read(subdir_path.join("repodata_shards.msgpack.zst")).unwrap();
    let shard_index_bytes = zstd::decode_all(shard_index_bytes.as_slice()).unwrap();
    let shard_index: ShardedRepodata = rmp_serde::from_slice(&shard_index_bytes).unwrap();
    let digest = shard_index.shards["empty"];
    let shard_bytes = fs::read(
        subdir_path
            .join("shards")
            .join(format!("{}.msgpack.zst", hex::encode(digest))),
    )
    .unwrap();
    let shard_bytes = zstd::decode_all(shard_bytes.as_slice()).unwrap();
    let shard: rattler_conda_types::Shard = rmp_serde::from_slice(&shard_bytes).unwrap();
    let record = shard
        .conda_packages
        .values()
        .next()
        .expect("shard contains the package");
    let indexed_timestamp = record
        .indexed_timestamp
        .expect("indexed_timestamp is present in the shard")
        .timestamp_millis();
    assert!(indexed_timestamp >= test_start_millis);
}
