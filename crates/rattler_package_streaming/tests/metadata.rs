use std::{io::Cursor, path::Path};

use rattler_conda_types::package::{CondaArchiveType, IndexJson};
use rattler_digest::{Md5, Sha256, compute_bytes_digest};
use rattler_package_streaming::{
    ExtractError, ExtractResult,
    read::PackageMetadata,
    tokio::async_read::{process_blocking, read_package_metadata},
};

fn assert_digest(result: &ExtractResult, bytes: &[u8]) {
    assert_eq!(result.sha256, compute_bytes_digest::<Sha256>(bytes));
    assert_eq!(result.md5, compute_bytes_digest::<Md5>(bytes));
    assert_eq!(result.total_size, bytes.len() as u64);
}

#[tokio::test]
async fn metadata_readers_agree_with_seekable_reader() {
    for (filename, archive_type) in [
        (
            "clobber-1-0.1.0-h4616a5c_0.tar.bz2",
            CondaArchiveType::TarBz2,
        ),
        (
            "clobber-fd-1-0.1.0-h4616a5c_0.conda",
            CondaArchiveType::Conda,
        ),
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/clobber")
            .join(filename);
        let bytes = std::fs::read(&path).unwrap();
        let expected: IndexJson =
            rattler_package_streaming::seek::read_package_file(&path).unwrap();
        let (sync_metadata, sync_digest) =
            PackageMetadata::from_reader(Cursor::new(&bytes), archive_type).unwrap();
        let (async_metadata, async_digest) =
            read_package_metadata(Cursor::new(&bytes), archive_type)
                .await
                .unwrap();
        assert_eq!(sync_metadata.index, expected);
        assert_eq!(async_metadata.index, expected);
        assert_eq!(sync_metadata.run_exports, async_metadata.run_exports);
        assert_digest(&sync_digest, &bytes);
        assert_digest(&async_digest, &bytes);

        if archive_type == CondaArchiveType::Conda {
            let (buffered, digest) =
                PackageMetadata::from_conda_via_buffering(Cursor::new(&bytes)).unwrap();
            assert_eq!(buffered.index, expected);
            assert_eq!(buffered.run_exports, sync_metadata.run_exports);
            assert_digest(&digest, &bytes);
        }
    }
}

#[tokio::test]
async fn blocking_processor_hashes_bytes_after_closure_returns() {
    // Exceed the pump's channel capacity so it must keep reading after the
    // processor returns its value, rather than relying on prefetched bytes.
    let bytes = vec![42; 8 * 1024 * 1024];
    let (value, digest) = process_blocking(Cursor::new(&bytes), |reader| {
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        Ok(byte[0])
    })
    .await
    .unwrap();
    assert_eq!(value, 42);
    assert_digest(&digest, &bytes);
}

#[tokio::test]
async fn blocking_processor_reports_errors_during_final_drain() {
    // The processor does not read anything. Draining must still surface the
    // source error instead of returning a digest for a truncated stream.
    let stream = futures::stream::iter([
        Ok(bytes::Bytes::from(vec![42; 1024 * 1024])),
        Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "source failed",
        )),
    ]);
    let error = process_blocking(tokio_util::io::StreamReader::new(stream), |_| Ok(()))
        .await
        .unwrap_err();
    assert!(matches!(error, ExtractError::IoError(ref error)
        if error.kind() == std::io::ErrorKind::ConnectionReset));
}

#[tokio::test]
async fn conda_metadata_preserves_run_exports_in_either_member_order() {
    use std::io::Write;

    let index = br#"{"name":"demo","version":"1","build":"0","build_number":0}"#;
    let exports = br#"{"weak":["libdemo >=1"]}"#;
    let mut tar = tar::Builder::new(Vec::new());
    for (name, content) in [
        ("info/index.json", index.as_slice()),
        ("info/run_exports.json", exports.as_slice()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, name, content).unwrap();
    }
    let info = zstd::encode_all(tar.into_inner().unwrap().as_slice(), 0).unwrap();
    // Exceed the spool's memory threshold. The payload is skipped, so its
    // contents do not need to be a compressed tar archive.
    let payload = vec![42; 6 * 1024 * 1024];
    for info_first in [true, false] {
        let mut members = [
            ("info-demo.tar.zst", info.as_slice()),
            ("pkg-demo.tar.zst", payload.as_slice()),
        ];
        if !info_first {
            members.reverse();
        }
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in members {
            zip.start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
            zip.write_all(bytes).unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();
        let (streamed, digest) =
            PackageMetadata::from_reader(Cursor::new(&bytes), CondaArchiveType::Conda).unwrap();
        assert_digest(&digest, &bytes);
        let (buffered, digest) =
            PackageMetadata::from_conda_via_buffering(Cursor::new(&bytes)).unwrap();
        assert_digest(&digest, &bytes);
        let (async_streamed, digest) =
            read_package_metadata(Cursor::new(&bytes), CondaArchiveType::Conda)
                .await
                .unwrap();
        assert_digest(&digest, &bytes);
        for metadata in [streamed, buffered, async_streamed] {
            assert_eq!(metadata.index.name.as_normalized(), "demo");
            assert_eq!(
                serde_json::to_value(metadata.run_exports.unwrap()).unwrap(),
                serde_json::from_slice::<serde_json::Value>(exports).unwrap(),
            );
        }
    }
}
