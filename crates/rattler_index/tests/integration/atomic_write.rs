use rattler_conda_types::{Channel, ChannelConfig, Platform};
use rattler_index::{IndexFsConfig, PackageRevisionAssignment, index_fs};
use rattler_repodata_gateway::sparse::SparseRepoData;

/// Indexing must succeed while `repodata.json` is memory-mapped (as the gateway
/// maps it). Atomic writes rename over the file instead of truncating it, which
/// on Windows would fail with `ERROR_USER_MAPPED_FILE`.
#[tokio::test]
async fn test_index_fs_over_memory_mapped_repodata() {
    let temp_dir = tempfile::tempdir().unwrap();
    let channel_path = temp_dir.path().to_path_buf();

    let make_config = || IndexFsConfig {
        channel: channel_path.clone(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        force: true,
        max_parallel: 8,
        multi_progress: None,
    };

    // Generate the initial repodata.
    index_fs(make_config())
        .await
        .expect("initial indexing should succeed");

    let repodata_path = channel_path.join("noarch").join("repodata.json");
    assert!(
        repodata_path.exists(),
        "expected {repodata_path:?} to exist"
    );

    // Map it like the gateway does and keep the mapping alive across re-indexing.
    let channel_config = ChannelConfig::default_with_root_dir(channel_path.clone());
    let channel = Channel::from_str("dummy", &channel_config).unwrap();
    let sparse = SparseRepoData::from_file(channel, "noarch", &repodata_path, None)
        .expect("mapping repodata.json should succeed");

    // Must succeed while the file is still mapped.
    index_fs(make_config())
        .await
        .expect("re-indexing over a mapped repodata.json should succeed");

    assert!(
        channel_path.join(".tmp").exists(),
        "atomic write dir expected"
    );

    drop(sparse);
}
