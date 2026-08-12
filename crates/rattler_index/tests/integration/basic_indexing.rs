use std::{
    collections::HashMap,
    fs,
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rattler_conda_types::{
    Channel, ChannelNotice, ChannelNoticeLevel, ChannelRelations, PackageName, Platform,
    RepoData as CondaRepoData, RepodataRevisionInfo, Shard, ShardedRepodata,
    compression_level::CompressionLevel,
};
use rattler_index::{
    ChannelMetadata, IndexFsConfig, PackageRevisionAssignment, RepodataRevision,
    RepodataRevisionSelection, index_fs, index_fs_with_channel_metadata,
};
use rattler_package_streaming::write::{write_conda_package, write_tar_bz2_package};
use rattler_repodata_gateway::{DownloadReporter, Gateway, Reporter, UnsupportedRepodataRevision};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShardedIndexShape {
    info: serde::de::IgnoredAny,
    shards: serde::de::IgnoredAny,
}

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
    assert_eq!(
        repodata_json
            .get("packages.conda")
            .unwrap()
            .get("conda-22.11.1-py38haa244fe_1.conda")
            .unwrap(),
        &expected_repodata_entry
    );
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

/// Rejects unsupported configured revisions before creating output for an empty channel.
#[tokio::test]
async fn test_empty_channel_rejects_unsupported_configured_revision() {
    let temp_dir = tempfile::tempdir().unwrap();

    let err = index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
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
    })
    .await
    .unwrap_err();

    assert!(err.to_string().contains(
        "repodata revision v4 cannot be configured; only v3 is selectable and the legacy layout is implicit"
    ));
    assert!(!temp_dir.path().join("noarch/repodata.json").exists());
}

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
async fn test_normal_and_force_reindex_preserve_v3_extensions() {
    let temp_dir = tempfile::tempdir().unwrap();
    let subdir_path = temp_dir.path().join("noarch");
    let package_name = "empty-0.1.0-h4616a5c_0.conda";
    fs::create_dir(&subdir_path).unwrap();
    fs::copy(
        test_data_dir().join("packages").join(package_name),
        subdir_path.join(package_name),
    )
    .unwrap();

    index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: true,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        force: false,
        max_parallel: 1,
        multi_progress: None,
    })
    .await
    .unwrap();

    let repodata_path = subdir_path.join("repodata.json");
    let mut repodata: Value = serde_json::from_reader(File::open(&repodata_path).unwrap()).unwrap();
    let extensions = serde_json::json!({
        "zip": {
            "empty-0.1.0-h4616a5c_0": { "future_metadata": ["preserve", true] }
        },
        "future-array": ["opaque", { "nested-null": null }],
        "future-scalar": "opaque",
        "future-null": null
    });
    repodata
        .as_object_mut()
        .unwrap()
        .insert("v3".to_string(), extensions.clone());
    // A forced reindex must not reuse package records from this source file.
    repodata["packages.conda"][package_name]["build"] = Value::String("stale".to_string());
    fs::write(&repodata_path, serde_json::to_vec(&repodata).unwrap()).unwrap();

    index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        force: false,
        max_parallel: 1,
        multi_progress: None,
    })
    .await
    .unwrap();
    let normally_reindexed: Value =
        serde_json::from_reader(File::open(&repodata_path).unwrap()).unwrap();
    assert_eq!(
        normally_reindexed["packages.conda"][package_name]["build"].as_str(),
        Some("stale")
    );
    assert_eq!(normally_reindexed["v3"], extensions);

    index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: true,
        write_shards: true,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        force: true,
        max_parallel: 1,
        multi_progress: None,
    })
    .await
    .unwrap();

    let reindexed: Value = serde_json::from_reader(File::open(&repodata_path).unwrap()).unwrap();
    assert_ne!(
        reindexed["packages.conda"][package_name]["build"].as_str(),
        Some("stale")
    );
    assert_eq!(reindexed["v3"], extensions);

    let compressed_repodata = fs::read(subdir_path.join("repodata.json.zst")).unwrap();
    let compressed_repodata: Value =
        serde_json::from_slice(&zstd::decode_all(compressed_repodata.as_slice()).unwrap()).unwrap();
    assert_eq!(compressed_repodata["v3"], extensions);

    let shard_index_bytes = fs::read(subdir_path.join("repodata_shards.msgpack.zst")).unwrap();
    let shard_index_bytes = zstd::decode_all(shard_index_bytes.as_slice()).unwrap();
    let shape: ShardedIndexShape = rmp_serde::from_slice(&shard_index_bytes).unwrap();
    let _ = (shape.info, shape.shards);
}

#[tokio::test]
async fn test_reindex_derives_authoritative_legacy_and_v3_stats_and_message_precedence() {
    let temp_dir = tempfile::tempdir().unwrap();
    let subdir_path = temp_dir.path().join("noarch");
    fs::create_dir(&subdir_path).unwrap();

    let package_build_dir = temp_dir.path().join("package-build");
    for (filename, index_json) in [
        (
            "legacy-stats-1.0-0.tar.bz2",
            r#"{
                "build": "0",
                "build_number": 0,
                "name": "legacy-stats",
                "noarch": "generic",
                "subdir": "noarch",
                "timestamp": 1710000000000,
                "version": "1.0"
            }"#,
        ),
        (
            "v3-stats-1.0-0.tar.bz2",
            r#"{
                "build": "0",
                "build_number": 0,
                "extra_depends": { "docs": ["sphinx"] },
                "name": "v3-stats",
                "noarch": "generic",
                "subdir": "noarch",
                "timestamp": 1720000000000,
                "version": "1.0"
            }"#,
        ),
    ] {
        let info_dir = package_build_dir.join("info");
        fs::create_dir_all(&info_dir).unwrap();
        fs::write(info_dir.join("index.json"), index_json).unwrap();
        write_tar_bz2_package(
            File::create(subdir_path.join(filename)).unwrap(),
            &package_build_dir,
            &[info_dir.join("index.json")],
            CompressionLevel::Default,
            None,
            None,
        )
        .unwrap();
        fs::remove_dir_all(&package_build_dir).unwrap();
    }

    let repodata_path = subdir_path.join("repodata.json");
    fs::write(
        &repodata_path,
        serde_json::to_vec(&serde_json::json!({
            "info": {
                "subdir": "noarch",
                "repodata_revisions": {
                    "v0": {
                        "message": "previous legacy message",
                        "n_packages": 99,
                        "oldest": 1,
                        "newest": 2
                    },
                    "v3": {
                        "message": "previous v3 message",
                        "n_packages": 99,
                        "oldest": 1,
                        "newest": 2
                    }
                }
            },
            "packages": {},
            "packages.conda": {},
            "repodata_version": 2
        }))
        .unwrap(),
    )
    .unwrap();

    index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        repodata_revisions: vec![RepodataRevisionSelection {
            revision: RepodataRevision::V3,
            message: Some("configured v3 message".to_string()),
        }],
        package_revision_assignment: PackageRevisionAssignment::FromIndexJson,
        force: true,
        max_parallel: 1,
        multi_progress: None,
    })
    .await
    .unwrap();

    let repodata: Value = serde_json::from_reader(File::open(repodata_path).unwrap()).unwrap();
    assert_eq!(
        repodata["info"]["repodata_revisions"],
        serde_json::json!({
            "v0": {
                "message": "previous legacy message",
                "n_packages": 1,
                "oldest": 1710000000000i64,
                "newest": 1710000000000i64
            },
            "v3": {
                "message": "configured v3 message",
                "n_packages": 1,
                "oldest": 1720000000000i64,
                "newest": 1720000000000i64
            }
        })
    );
}

#[tokio::test]
async fn test_force_reindex_with_patch_preserves_and_merge_patches_v3_extensions() {
    let temp_dir = tempfile::tempdir().unwrap();
    let subdir_path = temp_dir.path().join("noarch");
    let package_name = "empty-0.1.0-h4616a5c_0.conda";
    fs::create_dir(&subdir_path).unwrap();
    fs::copy(
        test_data_dir().join("packages").join(package_name),
        subdir_path.join(package_name),
    )
    .unwrap();

    index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        force: true,
        max_parallel: 1,
        multi_progress: None,
    })
    .await
    .unwrap();

    let repodata_path = subdir_path.join("repodata.json");
    let mut repodata: Value = serde_json::from_reader(File::open(&repodata_path).unwrap()).unwrap();
    repodata.as_object_mut().unwrap().insert(
        "v3".to_string(),
        serde_json::json!({
            "zip": {
                "unchanged": { "keep": true },
                "patched": {
                    "nested": { "keep": true, "remove": true },
                    "remove_me": true
                }
            },
            "future-null": null,
            "future-scalar": "original",
            "future-array": ["original"],
            "unchanged-null": null,
            "unchanged-scalar": "opaque",
            "unchanged-array": ["opaque", { "nested-null": null }]
        }),
    );
    fs::write(&repodata_path, serde_json::to_vec(&repodata).unwrap()).unwrap();

    let patch_source = temp_dir.path().join("patch-source");
    let patch_info_dir = patch_source.join("info");
    let patch_subdir = patch_source.join("noarch");
    fs::create_dir_all(&patch_info_dir).unwrap();
    fs::create_dir_all(&patch_subdir).unwrap();
    fs::write(
        patch_info_dir.join("index.json"),
        r#"{
            "build": "0",
            "build_number": 0,
            "name": "repodata-patches",
            "noarch": "generic",
            "subdir": "noarch",
            "version": "1.0"
        }"#,
    )
    .unwrap();
    fs::write(
        patch_subdir.join("patch_instructions.json"),
        serde_json::to_vec(&serde_json::json!({
            "v3": {
                "zip": {
                    "patched": {
                        "nested": { "added": true, "remove": null },
                        "remove_me": null
                    },
                    "added": { "fresh": true }
                },
                "future-null": { "from-null": true },
                "future-scalar": { "from-scalar": true },
                "future-array": ["replaced"]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let patch_name = "repodata-patches-1.0-0.conda";
    write_conda_package(
        File::create(subdir_path.join(patch_name)).unwrap(),
        &patch_source,
        &[
            patch_info_dir.join("index.json"),
            patch_subdir.join("patch_instructions.json"),
        ],
        CompressionLevel::Default,
        None,
        "repodata-patches-1.0-0",
        None,
        None,
    )
    .unwrap();

    index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: Some(patch_name.to_string()),
        write_zst: false,
        write_shards: false,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        force: true,
        max_parallel: 1,
        multi_progress: None,
    })
    .await
    .unwrap();

    let repodata: Value = serde_json::from_reader(File::open(repodata_path).unwrap()).unwrap();
    assert_eq!(
        repodata["v3"],
        serde_json::json!({
            "zip": {
                "unchanged": { "keep": true },
                "patched": { "nested": { "keep": true, "added": true } },
                "added": { "fresh": true }
            },
            "future-null": { "from-null": true },
            "future-scalar": { "from-scalar": true },
            "future-array": ["replaced"],
            "unchanged-null": null,
            "unchanged-scalar": "opaque",
            "unchanged-array": ["opaque", { "nested-null": null }]
        })
    );
}

async fn assert_invalid_multi_subdir_patch_is_preflighted(
    linux_patch: Value,
    expected_error: &str,
) {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut original_repodata = HashMap::new();
    for subdir in ["noarch", "linux-64"] {
        let subdir_path = temp_dir.path().join(subdir);
        fs::create_dir_all(&subdir_path).unwrap();
        let bytes = serde_json::to_vec(&serde_json::json!({
            "info": { "subdir": subdir },
            "packages": {},
            "packages.conda": {},
            "v3": {},
            "repodata_version": 1
        }))
        .unwrap();
        fs::write(subdir_path.join("repodata.json"), &bytes).unwrap();
        original_repodata.insert(subdir, bytes);
    }

    let patch_source = temp_dir.path().join("invalid-patch-source");
    let patch_info_dir = patch_source.join("info");
    let patch_noarch_dir = patch_source.join("noarch");
    let patch_linux_dir = patch_source.join("linux-64");
    fs::create_dir_all(&patch_info_dir).unwrap();
    fs::create_dir_all(&patch_noarch_dir).unwrap();
    fs::create_dir_all(&patch_linux_dir).unwrap();
    fs::write(
        patch_info_dir.join("index.json"),
        r#"{
            "build": "0",
            "build_number": 0,
            "name": "repodata-patches",
            "noarch": "generic",
            "subdir": "noarch",
            "version": "1.0"
        }"#,
    )
    .unwrap();
    fs::write(patch_noarch_dir.join("patch_instructions.json"), "{}").unwrap();
    fs::write(
        patch_linux_dir.join("patch_instructions.json"),
        serde_json::to_vec(&linux_patch).unwrap(),
    )
    .unwrap();

    let patch_name = "repodata-patches-1.0-0.conda";
    write_conda_package(
        File::create(temp_dir.path().join("noarch").join(patch_name)).unwrap(),
        &patch_source,
        &[
            patch_info_dir.join("index.json"),
            patch_noarch_dir.join("patch_instructions.json"),
            patch_linux_dir.join("patch_instructions.json"),
        ],
        CompressionLevel::Default,
        None,
        "repodata-patches-1.0-0",
        None,
        None,
    )
    .unwrap();

    let error = index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: None,
        repodata_patch: Some(patch_name.to_string()),
        write_zst: false,
        write_shards: false,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        force: true,
        max_parallel: 1,
        multi_progress: None,
    })
    .await
    .unwrap_err();
    let error = error.to_string();
    assert!(error.contains("invalid repodata patch for subdir linux-64"));
    assert!(error.contains(expected_error), "unexpected error: {error}");

    for (subdir, expected) in original_repodata {
        assert_eq!(
            fs::read(temp_dir.path().join(subdir).join("repodata.json")).unwrap(),
            expected,
            "{subdir} repodata changed before global patch preflight completed"
        );
    }
}

#[tokio::test]
async fn test_legacy_patch_preflight_rejects_invalid_subdir_before_any_subdir_write() {
    assert_invalid_multi_subdir_patch_is_preflighted(
        serde_json::json!({
            "packages": {
                "demo-1.0-0.tar.bz2": {
                    "extra_depends": { "test": ["pytest >=8"] }
                }
            }
        }),
        "legacy repodata patches cannot set extra_depends",
    )
    .await;
}

#[tokio::test]
async fn test_v3_patch_preflight_rejects_invalid_subdir_before_any_subdir_write() {
    assert_invalid_multi_subdir_patch_is_preflighted(
        serde_json::json!({
            "v3": {
                "conda": {
                    "demo-1.0-0": { "depends": ["python[version="] }
                }
            }
        }),
        "failed to parse depends MatchSpec",
    )
    .await;
}

#[tokio::test]
async fn test_reindex_rejects_unsupported_producer_map_without_rewriting() {
    let temp_dir = tempfile::tempdir().unwrap();
    let subdir_path = temp_dir.path().join("noarch");
    fs::create_dir(&subdir_path).unwrap();
    let repodata_path = subdir_path.join("repodata.json");
    let original = serde_json::json!({
        "info": { "subdir": "noarch" },
        "packages": {},
        "packages.conda": {},
        "v4": { "future": "data" },
        "repodata_version": 2
    });
    let original = serde_json::to_vec(&original).unwrap();
    fs::write(&repodata_path, &original).unwrap();

    let err = index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        force: true,
        max_parallel: 1,
        multi_progress: None,
    })
    .await
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("repodata producer map v4 is not supported by this indexer")
    );
    assert_eq!(fs::read(repodata_path).unwrap(), original);
}

#[tokio::test]
async fn test_reindex_rejects_v1_producer_map_without_rewriting() {
    let temp_dir = tempfile::tempdir().unwrap();
    let subdir_path = temp_dir.path().join("noarch");
    fs::create_dir(&subdir_path).unwrap();
    let repodata_path = subdir_path.join("repodata.json");
    let original = br#"{
        "info": { "subdir": "noarch" },
        "packages": {},
        "packages.conda": {},
        "v1": { "future": "data" },
        "repodata_version": 2
    }"#;
    fs::write(&repodata_path, original).unwrap();

    let err = index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        force: false,
        max_parallel: 1,
        multi_progress: None,
    })
    .await
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("repodata producer map v1 is not supported by this indexer")
    );
    assert_eq!(fs::read(repodata_path).unwrap(), original);
}

#[tokio::test]
async fn test_reindex_preserves_existing_revision_messages_until_overridden() {
    let temp_dir = tempfile::tempdir().unwrap();
    let subdir_path = temp_dir.path().join("noarch");
    fs::create_dir(&subdir_path).unwrap();
    let repodata_path = subdir_path.join("repodata.json");
    fs::write(
        &repodata_path,
        serde_json::to_vec(&serde_json::json!({
            "info": {
                "subdir": "noarch",
                "repodata_revisions": { "v3": { "message": "existing", "n_packages": 99 } }
            },
            "packages": {},
            "packages.conda": {},
            "repodata_version": 2
        }))
        .unwrap(),
    )
    .unwrap();

    for force in [false, true] {
        index_fs(IndexFsConfig {
            channel: temp_dir.path().into(),
            target_platform: Some(Platform::NoArch),
            repodata_patch: None,
            write_zst: false,
            write_shards: false,
            repodata_revisions: Vec::new(),
            package_revision_assignment: PackageRevisionAssignment::default(),
            force,
            max_parallel: 1,
            multi_progress: None,
        })
        .await
        .unwrap();
        let repodata: Value = serde_json::from_reader(File::open(&repodata_path).unwrap()).unwrap();
        assert_eq!(
            repodata["info"]["repodata_revisions"]["v3"]["message"],
            "existing"
        );
        assert_eq!(
            repodata["info"]["repodata_revisions"]["v3"]["n_packages"],
            0
        );
    }

    index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        repodata_revisions: vec![RepodataRevisionSelection {
            revision: RepodataRevision::V3,
            message: Some("caller".to_string()),
        }],
        package_revision_assignment: PackageRevisionAssignment::default(),
        force: false,
        max_parallel: 1,
        multi_progress: None,
    })
    .await
    .unwrap();
    let repodata: Value = serde_json::from_reader(File::open(repodata_path).unwrap()).unwrap();
    assert_eq!(
        repodata["info"]["repodata_revisions"]["v3"]["message"],
        "caller"
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
        repodata_revisions: vec![RepodataRevisionSelection {
            revision: RepodataRevision::V3,
            message: None,
        }],
        package_revision_assignment: PackageRevisionAssignment::Latest,
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

    let shard_digest = shard_index.shards["empty"];
    let shard_bytes = fs::read(
        subdir_path
            .join("shards")
            .join(format!("{}.msgpack.zst", hex::encode(shard_digest))),
    )
    .unwrap();
    let shard_bytes = zstd::decode_all(shard_bytes.as_slice()).unwrap();
    let shard: Shard = rmp_serde::from_slice(&shard_bytes).unwrap();
    assert!(
        shard
            .v3
            .conda
            .contains_key(&"empty-0.1.0-h4616a5c_0".parse().unwrap())
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
        repodata_revisions: vec![RepodataRevisionSelection {
            revision: RepodataRevision::V3,
            message: None,
        }],
        package_revision_assignment: PackageRevisionAssignment::FromIndexJson,
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
        notices: Some(vec![ChannelNotice {
            id: "security-1".to_string(),
            message: "Please update demo".to_string(),
            level: ChannelNoticeLevel::Critical,
            created_at: None,
            expires_at: None,
            interval: Some(24),
        }]),
    };

    index_fs_with_channel_metadata(
        IndexFsConfig {
            channel: temp_dir.path().into(),
            target_platform: Some(Platform::NoArch),
            repodata_patch: None,
            write_zst: true,
            write_shards: true,
            repodata_revisions: vec![RepodataRevisionSelection {
                revision: RepodataRevision::V3,
                message: None,
            }],
            package_revision_assignment: PackageRevisionAssignment::Latest,
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

    let notices_json: Value =
        serde_json::from_reader(File::open(temp_dir.path().join("notices.json")).unwrap()).unwrap();
    assert_eq!(notices_json["notices"][0]["id"], "security-1");
    assert_eq!(notices_json["notices"][0]["level"], "critical");
    assert_eq!(notices_json["notices"][0]["interval"], 24);
}

/// Regression test: sharded repodata must be reproducible.
///
/// Shard records were serialized in the random iteration order of an ahash
/// `HashMap`, so every index run produced a different shard digest for any
/// package name with more than one build. Since shards are content-addressed
/// and written `if_not_exists`, each reindex wrote a fresh shard file and
/// orphaned the old one — unbounded `shards/` growth with no package changes.
#[tokio::test]
async fn test_sharded_repodata_is_deterministic() {
    let temp_dir = tempfile::tempdir().unwrap();
    let subdir_path = temp_dir.path().join("noarch");
    fs::create_dir(&subdir_path).unwrap();

    // One package name with many builds => a single shard holding N records.
    // Two independent random orderings coincide with probability 1/N!, so
    // N = 10 (~3e-7) makes a non-deterministic indexer reliably write a fresh
    // shard file on reindex.
    let build_dir = temp_dir.path().join("build");
    for n in 0..10 {
        let info_dir = build_dir.join("info");
        fs::create_dir_all(&info_dir).unwrap();
        fs::write(
            info_dir.join("index.json"),
            format!(
                r#"{{"build":"h_{n}","build_number":{n},"name":"multi","noarch":"generic","subdir":"noarch","timestamp":1710000000000,"version":"1.0.0"}}"#
            ),
        )
        .unwrap();
        let pkg = subdir_path.join(format!("multi-1.0.0-h_{n}.tar.bz2"));
        write_tar_bz2_package(
            File::create(&pkg).unwrap(),
            &build_dir,
            &[info_dir.join("index.json")],
            CompressionLevel::Default,
            None,
            None,
        )
        .unwrap();
        fs::remove_dir_all(&build_dir).unwrap();
    }

    let config = |force| IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: true,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        force,
        max_parallel: 1,
        multi_progress: None,
    };

    // Build the index, then reindex the unchanged channel several times.
    index_fs(config(true)).await.unwrap();
    for _ in 0..3 {
        index_fs(config(false)).await.unwrap();
    }

    // Exactly one package name => exactly one live shard. A deterministic
    // indexer rewrites the same content-addressed file, so `shards/` holds a
    // single file; the bug leaves one new orphan per reshuffled reindex.
    let shard_files = fs::read_dir(subdir_path.join("shards"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".msgpack.zst"))
        .count();
    assert_eq!(
        shard_files, 1,
        "sharded repodata is non-deterministic: {shard_files} shard files after \
         reindexing an unchanged channel (expected 1)"
    );
}

#[tokio::test]
async fn test_cep_146_end_to_end_fixture() {
    #[derive(Clone)]
    struct RevisionReporter(Arc<Mutex<Vec<UnsupportedRepodataRevision>>>);

    impl Reporter for RevisionReporter {
        fn download_reporter(&self) -> Option<&dyn DownloadReporter> {
            None
        }

        fn on_unsupported_repodata_revision(&self, report: &UnsupportedRepodataRevision) {
            self.0.lock().unwrap().push(report.clone());
        }
    }

    let channel = tempfile::tempdir().unwrap();
    let subdir = channel.path().join("noarch");
    fs::create_dir(&subdir).unwrap();

    // Keep a legacy package alongside the v3 fixture to make sure the legacy maps
    // remain available and do not inherit v3-only fields.
    let legacy_filename = "empty-0.1.0-h4616a5c_0.conda";
    fs::copy(
        test_data_dir().join("packages").join(legacy_filename),
        subdir.join(legacy_filename),
    )
    .unwrap();

    let fixture_filename = "cep-fixture-1.0-0.tar.bz2";
    let fixture_build = channel.path().join("fixture-build");
    let fixture_info = fixture_build.join("info");
    fs::create_dir_all(&fixture_info).unwrap();
    let dependency = "python[version='>=3.10',build='*_cp*',channel='conda-forge',subdir='noarch']";
    fs::write(
        fixture_info.join("index.json"),
        serde_json::to_vec(&serde_json::json!({
            "build": "0",
            "build_number": 0,
            "constrains": ["python <3.13"],
            "depends": [dependency],
            "extra_depends": { "test": ["pytest >=8"] },
            "name": "cep-fixture",
            "noarch": "generic",
            "subdir": "noarch",
            "timestamp": 1710000000000i64,
            "version": "1.0"
        }))
        .unwrap(),
    )
    .unwrap();
    write_tar_bz2_package(
        File::create(subdir.join(fixture_filename)).unwrap(),
        &fixture_build,
        &[fixture_info.join("index.json")],
        CompressionLevel::Default,
        None,
        None,
    )
    .unwrap();

    let config = || IndexFsConfig {
        channel: channel.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        repodata_revisions: vec![RepodataRevisionSelection {
            revision: RepodataRevision::V3,
            message: Some("v3 fixture with optional dependencies".to_string()),
        }],
        package_revision_assignment: PackageRevisionAssignment::FromIndexJson,
        force: true,
        max_parallel: 1,
        multi_progress: None,
    };
    index_fs(config()).await.unwrap();

    let repodata_path = subdir.join("repodata.json");
    let mut repodata: Value = serde_json::from_reader(File::open(&repodata_path).unwrap()).unwrap();
    let fixture_identifier = "cep-fixture-1.0-0";
    let canonical_dependency = "python[version=\">=3.10\",build=\"*_cp*\",channel=\"https://conda.anaconda.org/conda-forge/\",subdir=\"noarch\"]";

    assert_eq!(repodata["repodata_version"], 1);
    assert_eq!(repodata["packages"], serde_json::json!({}));
    assert!(repodata["packages.conda"].get(legacy_filename).is_some());
    assert!(
        repodata["packages.conda"][legacy_filename]
            .get("extra_depends")
            .is_none()
    );
    assert!(repodata["v3"].get("conda").is_none());
    assert!(repodata["v3"].get("whl").is_none());
    let v3_record = &repodata["v3"]["tar.bz2"][fixture_identifier];
    assert_eq!(
        v3_record["depends"],
        serde_json::json!([canonical_dependency])
    );
    assert_eq!(
        v3_record["constrains"],
        serde_json::json!(["python[version=\"<3.13\"]"])
    );
    assert_eq!(
        v3_record["extra_depends"],
        serde_json::json!({ "test": ["pytest[version=\">=8\"]"] })
    );
    let revision = &repodata["info"]["repodata_revisions"]["v3"];
    assert_eq!(revision["message"], "v3 fixture with optional dependencies");
    assert_eq!(revision["n_packages"], 1);
    assert_eq!(revision["oldest"], 1710000000000i64);
    assert_eq!(revision["newest"], 1710000000000i64);
    assert!(revision.get("indexed_timestamp").is_none());

    let opaque_extension = serde_json::json!({
        "fixture": ["preserve", { "nested": null }]
    });
    repodata["v3"]["future-layout"] = opaque_extension.clone();
    fs::write(&repodata_path, serde_json::to_vec(&repodata).unwrap()).unwrap();

    // Reindexing must retain unknown v3 extension data while rebuilding the
    // typed records and their producer-owned statistics.
    index_fs(config()).await.unwrap();
    let mut repodata: Value = serde_json::from_reader(File::open(&repodata_path).unwrap()).unwrap();
    assert_eq!(repodata["v3"]["future-layout"], opaque_extension);
    assert_eq!(
        repodata["v3"]["tar.bz2"][fixture_identifier]["depends"],
        serde_json::json!([canonical_dependency])
    );

    // Future advertised revisions are reader metadata, not producer maps. The
    // permissive reader retains them and the gateway reports them while still
    // returning records from the legacy and v3 layouts it understands.
    repodata["info"]["repodata_revisions"]["v4"] = serde_json::json!({
        "message": "a future layout is available",
        "n_packages": 1
    });
    fs::write(&repodata_path, serde_json::to_vec(&repodata).unwrap()).unwrap();

    let mut reader_fixture = repodata.clone();
    reader_fixture["v3"]["conda"] = serde_json::json!({});
    reader_fixture["v3"]["whl"] = serde_json::json!({});
    let parsed: CondaRepoData = serde_json::from_value(reader_fixture).unwrap();
    assert!(parsed.v3.conda.is_empty());
    assert!(parsed.v3.whl.is_empty());
    assert_eq!(
        serde_json::to_value(&parsed).unwrap()["v3"]["future-layout"],
        repodata["v3"]["future-layout"]
    );
    let published_revision = serde_json::from_value::<RepodataRevisionInfo>(serde_json::json!({
        "revision": 3,
        "n_packages": 99
    }))
    .unwrap();
    assert_eq!(published_revision.n_packages, Some(99));

    let reports = Arc::new(Mutex::new(Vec::new()));
    let records = Gateway::new()
        .query(
            vec![Channel::try_from_directory(channel.path()).unwrap()],
            vec![Platform::NoArch],
            vec![
                PackageName::new_unchecked("empty"),
                PackageName::new_unchecked("cep-fixture"),
            ]
            .into_iter(),
        )
        .with_reporter(RevisionReporter(reports.clone()))
        .await
        .unwrap();
    let gateway_records: Vec<_> = records
        .iter()
        .flat_map(|repo_data| repo_data.iter())
        .collect();
    assert_eq!(gateway_records.len(), 2);

    let legacy_record = gateway_records
        .iter()
        .find(|record| record.identifier.to_file_name() == legacy_filename)
        .unwrap();
    assert!(legacy_record.package_record.extra_depends.is_empty());

    let v3_gateway_record = gateway_records
        .iter()
        .find(|record| record.identifier.to_file_name() == fixture_filename)
        .unwrap();
    assert_eq!(
        v3_gateway_record.package_record.depends,
        vec![canonical_dependency.to_string()]
    );
    assert_eq!(
        v3_gateway_record.package_record.constrains,
        vec!["python[version=\"<3.13\"]".to_string()]
    );
    assert_eq!(
        v3_gateway_record.package_record.extra_depends.get("test"),
        Some(&vec!["pytest[version=\">=8\"]".to_string()])
    );

    let reports = reports.lock().unwrap();
    assert!(reports.iter().any(|report| {
        report.subdir == "noarch"
            && report.revision == RepodataRevision::from(4)
            && report.supported_revision == RepodataRevision::V3
            && report.metadata.message.as_deref() == Some("a future layout is available")
    }));
}
