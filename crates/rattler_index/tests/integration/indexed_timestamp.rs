use std::{fs, fs::File, path::Path};

use rattler_conda_types::{
    Channel, ChannelConfig, PackageName, Platform, Shard, ShardedRepodata,
    compression_level::CompressionLevel,
};
use rattler_index::{
    IndexFsConfig, PackageRevisionAssignment, RepodataRevision, RepodataRevisionSelection,
    index_fs, package_record_from_archive,
};
use rattler_package_streaming::write::{write_conda_package, write_tar_bz2_package};
use rattler_repodata_gateway::sparse::{PackageFormatSelection, SparseRepoData};
use serde_json::{Value, json};

const BUILD_TIME: i64 = 1_600_000_000_000;
const PUBLISHED: i64 = 1_700_000_000_000;
const NAME: &str = "example-1.0-0";

fn archive(root: &Path, conda: bool, patch: bool) -> String {
    let source = tempfile::tempdir().unwrap();
    fs::create_dir_all(source.path().join("info")).unwrap();
    let index = source.path().join("info/index.json");
    fs::write(
        &index,
        serde_json::to_vec(&json!({
            "name": if patch { "patch" } else { "example" }, "version": "1.0", "build": "0",
            "build_number": 0, "subdir": "noarch", "timestamp": BUILD_TIME,
            // Archive metadata must never supply publication provenance.
            "indexed_timestamp": 42
        }))
        .unwrap(),
    )
    .unwrap();
    let name = if patch { "patch-1.0-0" } else { NAME };
    let filename = format!("{name}.{}", if conda { "conda" } else { "tar.bz2" });
    let mut files = vec![index];
    if patch {
        fs::create_dir_all(source.path().join("noarch")).unwrap();
        let instructions = source.path().join("noarch/patch_instructions.json");
        fs::write(
            &instructions,
            serde_json::to_vec(&json!({
                "packages": { format!("{NAME}.tar.bz2"): { "depends": ["python"] } }
            }))
            .unwrap(),
        )
        .unwrap();
        files.push(instructions);
    }
    fs::create_dir_all(root.join("noarch")).unwrap();
    let output = File::create(root.join("noarch").join(&filename)).unwrap();
    if conda {
        write_conda_package(
            output,
            source.path(),
            &files,
            CompressionLevel::Default,
            None,
            name,
            None,
            None,
        )
        .unwrap();
    } else {
        write_tar_bz2_package(
            output,
            source.path(),
            &files,
            CompressionLevel::Default,
            None,
            None,
        )
        .unwrap();
    }
    filename
}

async fn index(root: &Path, force: bool, patch: Option<&str>, v3: bool) -> Value {
    index_fs(IndexFsConfig {
        channel: root.into(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: patch.map(str::to_owned),
        write_zst: true,
        write_shards: true,
        write_lookup: false,
        repodata_revisions: if v3 {
            vec![RepodataRevisionSelection {
                revision: RepodataRevision::V3,
                message: None,
            }]
        } else {
            vec![]
        },
        package_revision_assignment: if v3 {
            PackageRevisionAssignment::Latest
        } else {
            PackageRevisionAssignment::default()
        },
        force,
        max_parallel: 2,
        multi_progress: None,
    })
    .await
    .unwrap();
    let bytes = fs::read(root.join("noarch/repodata.json")).unwrap();
    let compressed = fs::read(root.join("noarch/repodata.json.zst")).unwrap();
    assert_eq!(bytes, zstd::stream::decode_all(&compressed[..]).unwrap());
    serde_json::from_slice(&bytes).unwrap()
}

fn save(root: &Path, repodata: &Value) {
    fs::write(
        root.join("noarch/repodata.json"),
        serde_json::to_vec(repodata).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn publication_time_survives_reindex_and_archive_formats_are_independent() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let tar = archive(root, false, false);
    assert!(
        package_record_from_archive(&root.join("noarch").join(&tar))
            .unwrap()
            .indexed_timestamp
            .is_none()
    );
    let before = jiff::Timestamp::now().as_millisecond();
    let mut repodata = index(root, false, None, false).await;
    let timestamp = repodata["packages"][&tar]["indexed_timestamp"]
        .as_i64()
        .unwrap();
    assert!((before..=jiff::Timestamp::now().as_millisecond()).contains(&timestamp));
    assert_eq!(repodata["packages"][&tar]["timestamp"], BUILD_TIME);
    // Simulate a publication on a previous day without sleeping.
    repodata["packages"][&tar]["indexed_timestamp"] = json!(PUBLISHED);
    save(root, &repodata);
    let conda = archive(root, true, false);
    let mut conda_timestamp = None;
    for force in [false, true] {
        let repodata = index(root, force, None, false).await;
        assert_eq!(repodata["packages"][&tar]["indexed_timestamp"], PUBLISHED);
        let actual = repodata["packages.conda"][&conda]["indexed_timestamp"]
            .as_i64()
            .unwrap();
        assert!(actual >= before);
        assert_eq!(*conda_timestamp.get_or_insert(actual), actual);
    }
    let shard_index: ShardedRepodata = rmp_serde::from_slice(
        &zstd::stream::decode_all(
            &fs::read(root.join("noarch/repodata_shards.msgpack.zst")).unwrap()[..],
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(shard_index.shards.len(), 1);
    for hash in shard_index.shards.values() {
        let shard: Shard = rmp_serde::from_slice(
            &zstd::stream::decode_all(
                &fs::read(root.join(format!("noarch/shards/{}.msgpack.zst", hex::encode(hash))))
                    .unwrap()[..],
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(shard.packages.len(), 1);
        assert_eq!(shard.conda_packages.len(), 1);
        for record in shard.conda_packages.values() {
            assert_eq!(
                record.indexed_timestamp.map(|t| t.timestamp_millis()),
                conda_timestamp
            );
        }
        for record in shard.packages.values() {
            assert_eq!(
                record.indexed_timestamp.unwrap().timestamp_millis(),
                PUBLISHED
            );
        }
    }
    let channel =
        Channel::from_str("dummy", &ChannelConfig::default_with_root_dir(root.into())).unwrap();
    let sparse =
        SparseRepoData::from_file(channel, "noarch", root.join("noarch/repodata.json"), None)
            .unwrap();
    let records = sparse
        .load_records(
            &PackageName::new_unchecked("example"),
            PackageFormatSelection::Both,
        )
        .unwrap();
    assert_eq!(records.len(), 2);
    for record in records {
        let expected = if record.identifier.to_file_name() == tar {
            Some(PUBLISHED)
        } else {
            conda_timestamp
        };
        assert_eq!(
            record
                .package_record
                .indexed_timestamp
                .map(|t| t.timestamp_millis()),
            expected
        );
    }
    let migrated = index(root, true, None, true).await;
    let migrated: rattler_conda_types::RepoData = serde_json::from_value(migrated).unwrap();
    assert_eq!(
        migrated
            .v3
            .tar_bz2
            .values()
            .next()
            .unwrap()
            .indexed_timestamp
            .unwrap()
            .timestamp_millis(),
        PUBLISHED
    );
    let reindexed = index(root, false, None, true).await;
    let reindexed: rattler_conda_types::RepoData = serde_json::from_value(reindexed).unwrap();
    assert_eq!(
        reindexed
            .v3
            .tar_bz2
            .values()
            .next()
            .unwrap()
            .indexed_timestamp
            .unwrap()
            .timestamp_millis(),
        PUBLISHED
    );
}

#[tokio::test]
async fn patch_transitions_preserve_publication_history_including_legacy_missing_values() {
    for timestamp in [Some(PUBLISHED), None] {
        for force in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let tar = archive(root, false, false);
            let mut initial = index(root, false, None, false).await;
            if let Some(timestamp) = timestamp {
                initial["packages"][&tar]["indexed_timestamp"] = json!(timestamp);
            } else {
                initial["packages"][&tar]
                    .as_object_mut()
                    .unwrap()
                    .remove("indexed_timestamp");
            }
            save(root, &initial);
            let patch = archive(root, true, true);
            for patch_mode in [Some(patch.as_str()), Some(patch.as_str()), None] {
                let repodata = index(root, force, patch_mode, false).await;
                assert_eq!(
                    repodata["packages"][&tar]["indexed_timestamp"],
                    json!(timestamp)
                );
                if patch_mode.is_some() {
                    assert_eq!(repodata["packages"][&tar]["depends"], json!(["python"]));
                    let raw: Value = serde_json::from_slice(
                        &fs::read(root.join("noarch/repodata_from_packages.json")).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(raw["packages"][&tar]["indexed_timestamp"], json!(timestamp));
                    assert_ne!(raw["packages"][&tar]["depends"], json!(["python"]));
                }
            }
        }
    }
}

/// Publish a competing record immediately before our conditional write. Forced
/// retries must use its publication time even when reusing cached archive data.
#[tokio::test]
async fn forced_retry_preserves_concurrent_publication_with_cached_metadata() {
    use super::etag_memory_backend::{ETagMemoryBuilder, Operation, TestHooks};
    use opendal::Operator;
    use std::sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    let temp = tempfile::tempdir().unwrap();
    let filename = archive(temp.path(), true, false);
    let path = temp.path().join("noarch").join(&filename);
    let mut record = serde_json::to_value(package_record_from_archive(&path).unwrap()).unwrap();
    record["indexed_timestamp"] = json!(PUBLISHED);
    let competing = serde_json::to_vec(&json!({"packages.conda": { &filename: record }})).unwrap();
    let operator = Arc::new(OnceLock::<Operator>::new());
    let injected = Arc::new(AtomicBool::new(false));
    let archive_reads = Arc::new(AtomicUsize::new(0));
    let hooks = TestHooks {
        on_operation: Arc::new({
            let operator = operator.clone();
            let injected = injected.clone();
            let archive_reads = archive_reads.clone();
            move |path, operation| {
                let operator = operator.clone();
                let injected = injected.clone();
                let archive_reads = archive_reads.clone();
                let competing = competing.clone();
                let path = path.to_owned();
                Box::pin(async move {
                    if path.ends_with(".conda") && operation == Operation::BeforeRead {
                        archive_reads.fetch_add(1, Ordering::SeqCst);
                    }
                    if path == "noarch/repodata.json"
                        && operation == Operation::BeforeWrite
                        && !injected.swap(true, Ordering::SeqCst)
                    {
                        operator
                            .get()
                            .unwrap()
                            .write(&path, competing)
                            .await
                            .unwrap();
                    }
                })
            }
        }),
    };
    let op = Operator::new(ETagMemoryBuilder::default().with_test_hooks(hooks))
        .unwrap()
        .finish();
    operator.set(op.clone()).unwrap();
    op.write(&format!("noarch/{filename}"), fs::read(path).unwrap())
        .await
        .unwrap();
    let stats = rattler_index::index(
        Some(Platform::NoArch),
        op.clone(),
        None,
        false,
        false,
        false,
        vec![],
        PackageRevisionAssignment::default(),
        true,
        1,
        None,
        rattler_index::PreconditionChecks::Enabled,
    )
    .await
    .unwrap();
    assert_eq!(stats.subdirs.values().map(|s| s.retries).sum::<usize>(), 1);
    assert_eq!(
        archive_reads.load(Ordering::SeqCst),
        1,
        "retry should reuse cached archive metadata"
    );
    let repodata: Value =
        serde_json::from_slice(&op.read("noarch/repodata.json").await.unwrap().to_vec()).unwrap();
    assert_eq!(
        repodata["packages.conda"][&filename]["indexed_timestamp"],
        PUBLISHED
    );
}

#[tokio::test]
async fn disabling_removal_patch_preserves_raw_publication_time() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let tar = archive(root, false, false);
    let mut raw = index(root, false, None, false).await;
    raw["packages"][&tar]["indexed_timestamp"] = json!(PUBLISHED);
    fs::write(
        root.join("noarch/repodata_from_packages.json"),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
    let mut published = raw.clone();
    published["packages"].as_object_mut().unwrap().remove(&tar);
    published["removed"] = json!([&tar]);
    save(root, &published);
    let restored = index(root, false, None, false).await;
    assert_eq!(restored["packages"][&tar]["indexed_timestamp"], PUBLISHED);
}
