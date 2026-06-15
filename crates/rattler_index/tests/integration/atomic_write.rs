use rattler_conda_types::{Channel, ChannelConfig, Platform};
use rattler_index::{IndexFsConfig, PackageRevisionAssignment, index_fs};
use rattler_repodata_gateway::sparse::SparseRepoData;

/// Regression test for indexing a channel whose `repodata.json` is currently
/// memory-mapped, as the repodata gateway does via
/// [`SparseRepoData::from_file`].
///
/// On Windows, re-writing a memory-mapped file in place fails with
/// `ERROR_USER_MAPPED_FILE` (os error 1224). `index_fs` now writes through an
/// atomic temp dir on the same volume and renames over the target, so the
/// mapped file is replaced rather than truncated. Combined with the gateway
/// opening the file with `FILE_SHARE_DELETE`, the rename succeeds while the
/// mapping is still live.
///
/// On other platforms the rename is harmless and the test simply asserts that
/// indexing over a mapped file keeps working.
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

    // First pass: generate `noarch/repodata.json`.
    index_fs(make_config())
        .await
        .expect("initial indexing should succeed");

    let repodata_path = channel_path.join("noarch").join("repodata.json");
    assert!(
        repodata_path.exists(),
        "expected {repodata_path:?} to exist"
    );

    // Map the freshly generated repodata.json exactly like the gateway does and
    // keep the mapping alive across the second indexing pass.
    let channel_config = ChannelConfig::default_with_root_dir(channel_path.clone());
    let channel = Channel::from_str("dummy", &channel_config).unwrap();
    let sparse = SparseRepoData::from_file(channel, "noarch", &repodata_path, None)
        .expect("mapping repodata.json should succeed");

    // Second pass: must succeed even though the file is still memory-mapped.
    index_fs(make_config())
        .await
        .expect("re-indexing over a mapped repodata.json should succeed");

    // The atomic write dir lives inside the channel root but must not be treated
    // as a subdir.
    assert!(
        channel_path.join(".tmp").exists(),
        "atomic write dir should have been created inside the channel root"
    );

    // Keep the mapping alive until after the second write so the regression is
    // actually exercised.
    drop(sparse);
}
