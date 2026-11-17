//! Check archive layouts, whole-file digests, storage races and cancellation.

use super::*;
#[cfg(feature = "s3")]
mod http;
use crate::{
    PreconditionChecks,
    cache::{PackageRecordCache, read_package_record_with_retry},
    indexed_package_record_from_conda_reader, parse_package_buffer, read_and_parse_package,
};
use bytes::Bytes;
use opendal::{Configurator, services::FsConfig};
use rattler_conda_types::Subdir;
use serde_json::{json, to_value};
use std::{
    fs,
    io::{Cursor, Write, copy},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tar::{Builder as TarBuilder, Header};
use tempfile::tempdir;
use tokio::{
    runtime::Builder as RuntimeBuilder,
    spawn,
    sync::{Notify, OnceCell},
    time::timeout,
};
use tools::etag_memory_backend::{ETagMemoryBuilder, Operation, TestHooks};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};
use zstd::stream::encode_all;

/// Keep both metadata files in the fixture so equality checks catch lost run exports.
fn fixture(version: &str, info_first: bool, zip64: bool) -> Vec<u8> {
    fixture_with_payload(version, info_first, zip64, &[])
}

/// ZIP order, `Zip64` headers and the footer expose parser and whole-archive hash regressions.
fn fixture_with_payload(version: &str, info_first: bool, zip64: bool, payload: &[u8]) -> Vec<u8> {
    let index = json!({
        "name": "streaming-test", "version": version, "build": "0",
        "build_number": 0, "subdir": "noarch", "depends": ["python >=3.10"],
    })
    .to_string();
    let exports = r#"{"weak":["streaming-test >=1.0"]}"#;
    let mut tar = TarBuilder::new(Vec::new());
    for (path, content) in [
        ("info/run_exports.json", exports),
        ("info/index.json", index.as_str()),
    ] {
        let mut header = Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, path, content.as_bytes())
            .unwrap();
    }
    let info = encode_all(Cursor::new(tar.into_inner().unwrap()), 1).unwrap();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
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

/// Compare all published metadata and digests with the existing buffered reader.
#[track_caller]
fn assert_records_match(actual: &IndexedPackageRecord, expected: &IndexedPackageRecord) {
    assert_eq!(
        to_value(&actual.record).unwrap(),
        to_value(&expected.record).unwrap()
    );
}

/// Write a fixture and capture its identity before any test reads.
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

/// Replace the archive at a chosen read boundary and count matching read operations.
async fn replace_archive_on_read(
    builder: ETagMemoryBuilder,
    initial: Vec<u8>,
    replacement: Vec<u8>,
    trigger: (Operation, usize),
) -> (Operator, RepodataFileMetadata, Arc<AtomicUsize>) {
    let reads = Arc::new(AtomicUsize::new(0));
    let operator = Arc::new(OnceCell::<Operator>::new());
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

/// Check a storage read against the complete record produced by the buffered parser.
async fn assert_indexes_like_legacy(builder: ETagMemoryBuilder, bytes: Vec<u8>) {
    let expected = indexed_package_record_from_conda_reader(Cursor::new(&bytes)).unwrap();
    let (op, metadata) = operator_with_archive(builder, bytes).await;
    let (actual, _) = read_package_record_with_retry(&op, "test.conda", metadata)
        .await
        .unwrap();
    assert_records_match(&actual, &expected);
}

// Archive layouts and buffered fallbacks.

/// Preserve records for both component orders, `Zip64` headers and segmented buffers.
#[tokio::test]
async fn archive_layouts_match_the_buffered_reader() {
    for info_first in [false, true] {
        for zip64 in [false, true] {
            let bytes = fixture("1.0", info_first, zip64);
            for builder in [
                ETagMemoryBuilder::default(),
                ETagMemoryBuilder::default().without_range_reads(),
            ] {
                assert_indexes_like_legacy(builder, bytes.clone()).await;
            }
            let expected = indexed_package_record_from_conda_reader(Cursor::new(&bytes)).unwrap();
            for chunk_size in [1, 7, 64] {
                let buffer: Buffer = bytes
                    .chunks(chunk_size)
                    .map(Bytes::copy_from_slice)
                    .collect::<Vec<_>>()
                    .into();
                assert_records_match(&buffered_snapshot(buffer).unwrap(), &expected);
            }
        }
    }
}

/// Let the buffered parser handle duplicate info names instead of duplicating its selection rules.
#[tokio::test]
async fn duplicate_info_names_use_the_buffered_reader() {
    let source = fixture("1.0", true, false);
    let mut zip = ZipArchive::new(Cursor::new(&source)).unwrap();
    let mut output = ZipWriter::new(Cursor::new(Vec::new()));
    for name in ["info-test.tar.zst", "info-next.tar.zst"] {
        output
            .raw_copy_file_rename(zip.by_name("info-test.tar.zst").unwrap(), name)
            .unwrap();
    }
    let mut bytes = output.finish().unwrap().into_inner();
    // The writer rejects duplicates. Rename only the local/central header filenames.
    let offsets = {
        let mut zip = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let entry = zip.by_name("info-next.tar.zst").unwrap();
        [
            entry.header_start() as usize + 30,
            entry.central_header_start() as usize + 46,
        ]
    };
    for start in offsets {
        bytes[start..start + b"info-test.tar.zst".len()].copy_from_slice(b"info-test.tar.zst");
    }
    let (op, _) = operator_with_archive(ETagMemoryBuilder::default(), bytes.clone()).await;
    let reader = op.reader("test.conda").await.unwrap();
    let directory = op
        .reader_with("test.conda")
        .chunk(METADATA_CHUNK)
        .await
        .unwrap();
    assert!(matches!(
        metadata_bytes(&reader, directory, bytes.len() as u64).await,
        Err(MetadataReadError::UnsupportedLayout)
    ));
    assert_indexes_like_legacy(ETagMemoryBuilder::default(), bytes).await;
}

/// Keep support for streamed ZIP writers and comments accepted by the legacy parser
/// even when the range parser rejects their layout.
#[tokio::test]
async fn stored_data_descriptors_and_legacy_zip_comments_remain_readable() {
    for descriptors in [false, true] {
        let source = fixture("1.0", true, true);
        let mut zip = ZipArchive::new(Cursor::new(&source)).unwrap();
        let mut output = ZipWriter::new_stream(Vec::new());
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).unwrap();
            output
                .start_file(
                    file.name().unwrap(),
                    SimpleFileOptions::default()
                        .compression_method(CompressionMethod::Stored)
                        .large_file(true),
                )
                .unwrap();
            copy(&mut file, &mut output).unwrap();
        }
        let mut bytes = if descriptors {
            output.finish().unwrap().into_inner()
        } else {
            source
        };
        if !descriptors {
            // A control byte in the comment is accepted by the buffered ZIP reader.
            let comment = b"archive footer must be included in hashes";
            let offset = bytes.len() - comment.len();
            bytes[offset] = 1;
        }
        assert_indexes_like_legacy(ETagMemoryBuilder::default(), bytes).await;
    }
}

// Replacements must preserve the record's archive identity.

/// Equal timestamps and sizes cannot prove object identity. A replacement after the read
/// must leave hashes and metadata describing the same original snapshot.
#[tokio::test]
async fn storage_without_etags_uses_one_consistent_snapshot() {
    for timestamp_only in [false, true] {
        let mut initial = fixture("1.0", false, false);
        let mut replacement = fixture("2.0", false, false);
        let length = initial.len().max(replacement.len());
        initial.resize(length, 0);
        replacement.resize(length, 0);
        let expected = indexed_package_record_from_conda_reader(Cursor::new(&initial)).unwrap();
        let builder = if timestamp_only {
            ETagMemoryBuilder::default().with_timestamp_only()
        } else {
            ETagMemoryBuilder::default().without_identity()
        };
        let (op, metadata, reads) =
            replace_archive_on_read(builder, initial, replacement, (Operation::AfterRead, 0)).await;
        assert!(metadata.etag.is_none());
        assert_eq!(metadata.last_modified.is_some(), timestamp_only);
        let (actual, _) = read_package_record_with_retry(&op, "test.conda", metadata)
            .await
            .unwrap();
        assert_records_match(&actual, &expected);
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }
}

/// A replacement after hashing must restart both hashing and metadata parsing,
/// including when the backend exposes `ETags` but cannot enforce read conditions.
#[tokio::test]
async fn changed_archive_retries_the_whole_record_with_or_without_read_conditions() {
    for (builder, trigger) in [
        (ETagMemoryBuilder::default(), (Operation::BeforeRead, 1)),
        (
            ETagMemoryBuilder::default().without_conditional_reads(),
            (Operation::AfterRead, 0),
        ),
    ] {
        let replacement = fixture("2.0", false, false);
        let expected = indexed_package_record_from_conda_reader(Cursor::new(&replacement)).unwrap();
        let (op, metadata, reads) =
            replace_archive_on_read(builder, fixture("1.0", false, false), replacement, trigger)
                .await;
        let original_etag = metadata.etag.clone();
        let (actual, final_metadata) = read_package_record_with_retry(&op, "test.conda", metadata)
            .await
            .unwrap();
        assert_records_match(&actual, &expected);
        assert_ne!(final_metadata.etag, original_etag);
        assert!(reads.load(Ordering::SeqCst) >= 4);
    }
}

/// A replacement can invalidate ZIP offsets and cause decoding to fail before a record exists.
#[tokio::test]
async fn replacement_before_info_read_retries_instead_of_returning_a_parse_error() {
    let payload = encode_all(Cursor::new(vec![0; 4096]), 1).unwrap();
    let initial = fixture_with_payload("1.0", true, false, &payload);
    let replacement = fixture_with_payload("1.0", false, false, &payload);
    assert_eq!(initial.len(), replacement.len());
    let expected = indexed_package_record_from_conda_reader(Cursor::new(&replacement)).unwrap();

    // Count the baseline reads so the trigger follows the final info GET if ZIP seeks change.
    let (op, mut metadata, reads) = replace_archive_on_read(
        ETagMemoryBuilder::default().without_conditional_reads(),
        initial.clone(),
        replacement.clone(),
        (Operation::BeforeRead, usize::MAX),
    )
    .await;
    metadata.precondition_checks = PreconditionChecks::Disabled;
    read_package(&op, "test.conda", &metadata).await.unwrap();
    let last_read = reads.load(Ordering::SeqCst) - 1;

    let (op, metadata, _) = replace_archive_on_read(
        ETagMemoryBuilder::default().without_conditional_reads(),
        initial,
        replacement,
        (Operation::BeforeRead, last_read),
    )
    .await;
    let original_etag = metadata.etag.clone();
    let (actual, final_metadata) = read_package_record_with_retry(&op, "test.conda", metadata)
        .await
        .unwrap();
    assert_records_match(&actual, &expected);
    assert_ne!(final_metadata.etag, original_etag);
}

// Cancellation and executor safety.

/// Filesystem I/O also uses the blocking pool, so parsing must not hold its only worker during reads.
#[test]
fn filesystem_indexing_finishes_with_one_blocking_thread() {
    let bytes = fixture("1.0", false, false);
    let root = tempdir().unwrap();
    fs::write(root.path().join("test.conda"), &bytes).unwrap();
    let mut config = FsConfig::default();
    config.root = Some(root.path().to_str().unwrap().to_owned());
    let op = Operator::new(config.into_builder()).unwrap();
    let runtime = RuntimeBuilder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    let result = runtime.block_on(async {
        let metadata = RepodataFileMetadata::new(&op, "test.conda", PreconditionChecks::Enabled)
            .await
            .unwrap();
        timeout(
            Duration::from_secs(2),
            read_package_record_with_retry(&op, "test.conda", metadata),
        )
        .await
    });
    // Keep a regression from hanging the test process during runtime drop.
    runtime.shutdown_timeout(Duration::from_millis(50));
    assert!(
        result
            .expect("filesystem I/O deadlocked on the blocking pool")
            .is_ok()
    );
}

/// Dropping indexing must cancel pending archive I/O, including later metadata ranges,
/// instead of leaving a blocking worker downloading an abandoned archive.
#[tokio::test]
async fn aborting_hash_or_metadata_reads_drops_the_pending_io() {
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    for blocked_read in [0, 1] {
        let reads = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
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
        let task = spawn(async move { read_package(&op, "test.conda", &metadata).await });
        timeout(Duration::from_secs(2), entered.notified())
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

// Error kinds must survive the I/O adapters and archive parser.

/// Preserve replacement errors nested by `OpenDAL`'s futures reader and async ZIP.
#[test]
fn nested_storage_errors_preserve_the_retry_kind() {
    for depth in [1, 2] {
        let mut error = io::Error::other(Error::new(ErrorKind::ConditionNotMatch, "replaced"));
        for _ in 1..depth {
            error = io::Error::other(error);
        }
        assert_eq!(storage_error(error).kind(), ErrorKind::ConditionNotMatch);
    }
}

/// Keep the original parser's error kind and message for corrupt tar.bz2 packages.
#[tokio::test]
async fn corrupt_tar_bz2_preserves_the_parser_error() {
    let bytes = Bytes::from_static(b"not a bzip2 archive");
    let expected = parse_package_buffer(Buffer::from(bytes.clone()), "broken.tar.bz2").unwrap_err();
    let op = Operator::new(ETagMemoryBuilder::default()).unwrap();
    op.write("noarch/broken.tar.bz2", bytes).await.unwrap();
    let actual = read_and_parse_package(
        &op,
        &PackageRecordCache::new(),
        Subdir::NoArch,
        "broken.tar.bz2",
    )
    .await
    .unwrap_err();
    assert_eq!(actual.kind(), expected.kind());
    assert_eq!(actual.to_string(), expected.to_string());
}
