use super::*;
#[cfg(feature = "s3")]
mod http;
use crate::{
    PreconditionChecks,
    streaming_test_backend::{ETagMemoryBuilder, Operation, TestHooks},
};
use opendal::Configurator;
use std::{
    io::{Cursor, Write},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn fixture(version: &str, info_first: bool, zip64: bool) -> Vec<u8> {
    fixture_with_payload(version, info_first, zip64, &[])
}

fn fixture_with_payload(version: &str, info_first: bool, zip64: bool, payload: &[u8]) -> Vec<u8> {
    let index = serde_json::json!({
        "name": "streaming-test", "version": version, "build": "0",
        "build_number": 0, "subdir": "noarch", "depends": ["python >=3.10"],
    })
    .to_string();
    let exports = r#"{"weak":["streaming-test >=1.0"]}"#;
    let mut tar = tar::Builder::new(Vec::new());
    for (path, content) in [
        ("info/run_exports.json", exports),
        ("info/index.json", index.as_str()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, path, content.as_bytes())
            .unwrap();
    }
    let info = zstd::stream::encode_all(Cursor::new(tar.into_inner().unwrap()), 1).unwrap();
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(zip64);
    let mut entries = vec![
        ("info-test.tar.zst", info),
        ("pkg-test.tar.zst", payload.to_vec()),
    ];
    if !info_first {
        entries.reverse();
    }
    for (name, bytes) in entries {
        zip.start_file(name, options).unwrap();
        zip.write_all(&bytes).unwrap();
    }
    zip.set_comment("archive footer must be included in hashes")
        .unwrap();
    zip.finish().unwrap().into_inner()
}

#[track_caller]
fn assert_records_match(actual: &IndexedPackageRecord, expected: &IndexedPackageRecord) {
    assert_eq!(
        serde_json::to_value(&actual.record).unwrap(),
        serde_json::to_value(&expected.record).unwrap()
    );
}

async fn operator_with_archive(
    builder: ETagMemoryBuilder,
    bytes: Vec<u8>,
) -> (Operator, RepodataFileMetadata) {
    let op = Operator::new(builder).unwrap();
    op.write("test.conda", bytes).await.unwrap();
    let metadata = RepodataFileMetadata::new(&op, "test.conda", PreconditionChecks::Enabled)
        .await
        .unwrap();
    (op, metadata)
}

async fn replace_archive_on_read(
    builder: ETagMemoryBuilder,
    initial: Vec<u8>,
    replacement: Vec<u8>,
    trigger: (Operation, usize),
) -> (Operator, RepodataFileMetadata, Arc<AtomicUsize>) {
    let reads = Arc::new(AtomicUsize::new(0));
    let operator = Arc::new(tokio::sync::OnceCell::<Operator>::new());
    let hooks = TestHooks {
        on_operation: Arc::new({
            let operator = operator.clone();
            let reads = reads.clone();
            move |path, operation| {
                let operator = operator.clone();
                let replacement = replacement.clone();
                let replace = path == "test.conda"
                    && operation == trigger.0
                    && reads.fetch_add(1, Ordering::SeqCst) == trigger.1;
                Box::pin(async move {
                    if replace {
                        operator
                            .get()
                            .unwrap()
                            .write("test.conda", replacement)
                            .await
                            .unwrap();
                    }
                })
            }
        }),
    };
    let (op, metadata) = operator_with_archive(builder.with_test_hooks(hooks), initial).await;
    operator.set(op.clone()).unwrap();
    (op, metadata, reads)
}

#[tokio::test]
async fn metadata_and_full_archive_hashes_match_for_both_orders_and_zip64() {
    for info_first in [false, true] {
        for zip64 in [false, true] {
            let bytes = fixture("1.0", info_first, zip64);
            let expected =
                crate::indexed_package_record_from_conda_reader(Cursor::new(&bytes)).unwrap();
            let (op, metadata) = operator_with_archive(ETagMemoryBuilder::default(), bytes).await;
            let (actual, _) =
                crate::cache::read_package_record_with_retry(&op, "test.conda", metadata)
                    .await
                    .unwrap();
            assert_records_match(&actual, &expected);
        }
    }
}

#[tokio::test]
async fn duplicate_info_names_match_legacy_selection() {
    for duplicate_last in [false, true] {
        let mut entries = [
            ("info-test.tar.zst", "1.0"),
            ("info-last.tar.zst", "2.0"),
            ("info-next.tar.zst", "3.0"),
        ];
        if duplicate_last {
            entries.swap(1, 2);
        }
        let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, version) in entries {
            let source = fixture(version, true, false);
            let mut zip = zip::ZipArchive::new(Cursor::new(source)).unwrap();
            output
                .raw_copy_file_rename(zip.by_name("info-test.tar.zst").unwrap(), name)
                .unwrap();
        }
        let mut bytes = output.finish().unwrap().into_inner();
        // The writer rejects duplicates, so rename the local and central headers afterward.
        let mut renamed = 0;
        for offset in 0..=bytes.len() - b"info-last.tar.zst".len() {
            if bytes[offset..].starts_with(b"info-last.tar.zst") {
                bytes[offset..offset + b"info-last.tar.zst".len()]
                    .copy_from_slice(b"info-test.tar.zst");
                renamed += 1;
            }
        }
        assert_eq!(renamed, 2);
        let expected =
            crate::indexed_package_record_from_conda_reader(Cursor::new(&bytes)).unwrap();
        assert_eq!(expected.record.version.to_string(), "2.0");
        let size = bytes.len() as u64;
        let (op, metadata) = operator_with_archive(ETagMemoryBuilder::default(), bytes).await;
        let reader = op.reader("test.conda").await.unwrap();
        let directory = op
            .reader_with("test.conda")
            .chunk(METADATA_CHUNK)
            .await
            .unwrap();
        // Compatibility must not require a whole-archive snapshot for duplicates.
        metadata_bytes(&reader, directory, size).await.unwrap();

        let actual = read_package(&op, "test.conda", &metadata).await.unwrap();
        assert_records_match(&actual, &expected);
    }
}

#[tokio::test]
async fn changed_archive_between_hashing_and_metadata_retries_the_whole_record() {
    let replacement = fixture("2.0", false, false);
    let (op, metadata, reads) = replace_archive_on_read(
        ETagMemoryBuilder::default(),
        fixture("1.0", false, false),
        replacement.clone(),
        (Operation::BeforeRead, 1),
    )
    .await;
    let original_etag = metadata.etag.clone();
    let (actual, final_metadata) =
        crate::cache::read_package_record_with_retry(&op, "test.conda", metadata)
            .await
            .unwrap();
    let expected =
        crate::indexed_package_record_from_conda_reader(Cursor::new(&replacement)).unwrap();
    assert_records_match(&actual, &expected);
    assert_ne!(final_metadata.etag, original_etag);
    assert!(reads.load(Ordering::SeqCst) >= 4);
}

#[test]
fn filesystem_indexing_finishes_with_one_blocking_thread() {
    let bytes = fixture("1.0", false, false);
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("test.conda"), &bytes).unwrap();
    let mut config = opendal::services::FsConfig::default();
    config.root = Some(root.path().to_str().unwrap().to_owned());
    let op = Operator::new(config.into_builder()).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    let result = runtime.block_on(async {
        let metadata = RepodataFileMetadata::new(&op, "test.conda", PreconditionChecks::Enabled)
            .await
            .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            crate::cache::read_package_record_with_retry(&op, "test.conda", metadata),
        )
        .await
    });
    // Keep a regression from hanging the test process during runtime drop.
    runtime.shutdown_timeout(std::time::Duration::from_millis(50));
    assert!(
        result
            .expect("filesystem I/O deadlocked on the blocking pool")
            .is_ok()
    );
}

#[tokio::test]
async fn storage_without_etags_uses_one_consistent_snapshot() {
    for timestamp_only in [false, true] {
        let mut initial = fixture("1.0", false, false);
        let mut replacement = fixture("2.0", false, false);
        let length = initial.len().max(replacement.len());
        initial.resize(length, 0);
        replacement.resize(length, 0);
        let expected =
            crate::indexed_package_record_from_conda_reader(Cursor::new(&initial)).unwrap();
        let builder = if timestamp_only {
            ETagMemoryBuilder::default().with_timestamp_only()
        } else {
            ETagMemoryBuilder::default().without_identity()
        };
        let (op, metadata, reads) =
            replace_archive_on_read(builder, initial, replacement, (Operation::AfterRead, 0)).await;
        assert!(metadata.etag.is_none());
        assert_eq!(metadata.last_modified.is_some(), timestamp_only);
        let (actual, _) = crate::cache::read_package_record_with_retry(&op, "test.conda", metadata)
            .await
            .unwrap();
        assert_records_match(&actual, &expected);
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn detected_replacement_retries_without_conditional_request_support() {
    let replacement = fixture("2.0", false, false);
    let expected =
        crate::indexed_package_record_from_conda_reader(Cursor::new(&replacement)).unwrap();
    let (op, metadata, _) = replace_archive_on_read(
        ETagMemoryBuilder::default().without_conditional_reads(),
        fixture("1.0", false, false),
        replacement,
        (Operation::AfterRead, 0),
    )
    .await;
    let original_etag = metadata.etag.clone();
    let (actual, final_metadata) =
        crate::cache::read_package_record_with_retry(&op, "test.conda", metadata)
            .await
            .unwrap();
    assert_records_match(&actual, &expected);
    assert_ne!(final_metadata.etag, original_etag);
}

#[tokio::test]
async fn aborting_hash_or_metadata_reads_drops_the_pending_io() {
    struct Dropped(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    for blocked_read in [0, 1] {
        let reads = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let hooks = TestHooks {
            on_operation: Arc::new({
                let reads = reads.clone();
                let dropped = dropped.clone();
                let entered = entered.clone();
                let release = release.clone();
                move |path, operation| {
                    let block = path == "test.conda"
                        && operation == Operation::BeforeRead
                        && reads.fetch_add(1, Ordering::SeqCst) == blocked_read;
                    let dropped = dropped.clone();
                    let entered = entered.clone();
                    let release = release.clone();
                    Box::pin(async move {
                        if block {
                            let _guard = Dropped(dropped);
                            entered.notify_one();
                            release.notified().await;
                        }
                    })
                }
            }),
        };
        let (op, metadata) = operator_with_archive(
            ETagMemoryBuilder::default().with_test_hooks(hooks),
            fixture("1.0", false, false),
        )
        .await;
        let task = tokio::spawn(async move { read_package(&op, "test.conda", &metadata).await });
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let cancelled_io = dropped.load(Ordering::SeqCst);
        // Release an old non-cancellable worker too, so a failure is bounded.
        release.notify_one();
        assert!(cancelled_io, "archive I/O outlived its cancelled task");
    }
}

#[tokio::test]
async fn storage_without_range_reads_keeps_the_original_snapshot_path() {
    let bytes = fixture("1.0", false, false);
    let expected = crate::indexed_package_record_from_conda_reader(Cursor::new(&bytes)).unwrap();
    let (op, metadata) =
        operator_with_archive(ETagMemoryBuilder::default().without_range_reads(), bytes).await;
    let actual = read_package(&op, "test.conda", &metadata).await.unwrap();
    assert_records_match(&actual, &expected);
}

#[tokio::test]
async fn stored_data_descriptors_and_legacy_zip_comments_remain_readable() {
    use std::io::Read;
    for descriptors in [false, true] {
        let source = fixture("1.0", true, true);
        let mut zip = zip::ZipArchive::new(Cursor::new(&source)).unwrap();
        let mut output = zip::ZipWriter::new_stream(Vec::new());
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).unwrap();
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            output
                .start_file(
                    file.name().unwrap(),
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Stored)
                        .large_file(true),
                )
                .unwrap();
            output.write_all(&bytes).unwrap();
        }
        let mut bytes = if descriptors {
            output.finish().unwrap().into_inner()
        } else {
            source
        };
        if !descriptors {
            // The legacy reader permits this comment; async ZIP's stricter
            // embedded-record heuristic rejects it and must use the fallback.
            let comment = b"archive footer must be included in hashes";
            let offset = bytes.len() - comment.len();
            bytes[offset] = 1;
        }
        let expected =
            crate::indexed_package_record_from_conda_reader(Cursor::new(&bytes)).unwrap();
        let (op, metadata) = operator_with_archive(ETagMemoryBuilder::default(), bytes).await;

        let actual = read_package(&op, "test.conda", &metadata).await.unwrap();
        assert_records_match(&actual, &expected);
    }
}

#[test]
fn buffered_snapshot_handles_segment_boundaries_without_changing_records() {
    for info_first in [false, true] {
        for zip64 in [false, true] {
            let bytes = fixture("1.0", info_first, zip64);
            let expected =
                crate::indexed_package_record_from_conda_reader(Cursor::new(&bytes)).unwrap();
            for chunk_size in [1, 7, 64] {
                let buffer: Buffer = bytes
                    .chunks(chunk_size)
                    .map(bytes::Bytes::copy_from_slice)
                    .collect::<Vec<_>>()
                    .into();
                let actual = buffered_snapshot(buffer).unwrap();
                assert_records_match(&actual, &expected);
            }
        }
    }
}
