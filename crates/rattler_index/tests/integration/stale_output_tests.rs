use rattler_conda_types::Platform;
use rattler_index::{index_fs, IndexFsConfig};

/// When both zst and shards were previously enabled then both disabled,
/// the stale files must still exist on disk (they are not cleaned up).
/// Stale-file warnings must be emitted.
#[tracing_test::traced_test]
#[tokio::test]
async fn test_stale_files_persist_when_outputs_disabled() {
    let temp_dir = tempfile::tempdir().unwrap();

    // First run: generate both zst and shards files.
    index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: true,
        write_shards: true,
        force: true,
        max_parallel: 10,
        multi_progress: None,
    })
    .await
    .unwrap();

    let zst_path = temp_dir.path().join("noarch/repodata.json.zst");
    let shards_path = temp_dir.path().join("noarch/repodata_shards.msgpack.zst");
    assert!(zst_path.exists());
    assert!(shards_path.exists());

    // Second run: both disabled, stale files should still exist.
    index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        force: false,
        max_parallel: 10,
        multi_progress: None,
    })
    .await
    .unwrap();

    // Stale files are not cleaned up by the indexer.
    assert!(zst_path.exists());
    assert!(shards_path.exists());

    // Stale-file warnings must have been logged.
    assert!(logs_contain(
        "Found stale file 'noarch/repodata.json.zst' but zst output is disabled"
    ));
    assert!(logs_contain(
        "Found stale file 'noarch/repodata_shards.msgpack.zst' but sharded repodata output is disabled"
    ));
}

/// No stale files exist when both outputs are disabled from the start.
/// No stale-file warnings should be emitted.
#[tracing_test::traced_test]
#[tokio::test]
async fn test_no_stale_files_on_clean_start() {
    let temp_dir = tempfile::tempdir().unwrap();

    index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        force: true,
        max_parallel: 10,
        multi_progress: None,
    })
    .await
    .unwrap();

    assert!(!temp_dir.path().join("noarch/repodata.json.zst").exists());
    assert!(!temp_dir
        .path()
        .join("noarch/repodata_shards.msgpack.zst")
        .exists());

    // No stale files means no warnings.
    assert!(!logs_contain("Found stale file"));
}

/// Output files are kept up to date when both outputs remain enabled across runs.
/// No stale-file warnings should be emitted since outputs stay enabled.
#[tracing_test::traced_test]
#[tokio::test]
async fn test_outputs_updated_when_enabled() {
    let temp_dir = tempfile::tempdir().unwrap();

    index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: true,
        write_shards: true,
        force: true,
        max_parallel: 10,
        multi_progress: None,
    })
    .await
    .unwrap();

    let zst_path = temp_dir.path().join("noarch/repodata.json.zst");
    let shards_path = temp_dir.path().join("noarch/repodata_shards.msgpack.zst");
    assert!(zst_path.exists());
    assert!(shards_path.exists());

    index_fs(IndexFsConfig {
        channel: temp_dir.path().into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: true,
        write_shards: true,
        force: false,
        max_parallel: 10,
        multi_progress: None,
    })
    .await
    .unwrap();

    // Files still exist and were updated (not stale).
    assert!(zst_path.exists());
    assert!(shards_path.exists());

    // No stale-file warnings since both outputs remained enabled.
    assert!(!logs_contain("Found stale file"));
}
