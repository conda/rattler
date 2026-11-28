//! Check that the S3 client hashes with one full GET, then uses only bounded,
//! version-guarded metadata GETs with a small total transfer overhead.

use super::*;
mod server;
use server::TestServer;

// The fixtures normally need six metadata GETs. Allow limited reader read-ahead;
// Keep the response cap independent of the reader configuration under test.
const MAX_METADATA_GETS: usize = 11;
const METADATA_RESPONSE_LIMIT: usize = 64 * 1024;

/// The payload must be downloaded once for hashing. Metadata lookups may add only
/// bounded, version-guarded ranges regardless of where the info component appears.
#[tokio::test]
async fn s3_metadata_requests_are_bounded_and_transfer_only_small_ranges() {
    for info_first in [false, true] {
        // A large stored payload exposes an accidental second offset..EOF GET.
        let bytes = fixture_with_payload("1.0", info_first, true, &vec![0; 4 * 1024 * 1024]);
        let expected = indexed_package_record_from_conda_reader(Cursor::new(&bytes)).unwrap();
        let (info_start, info_end) = {
            let mut zip = ZipArchive::new(Cursor::new(&bytes)).unwrap();
            let info = zip.by_name("info-test.tar.zst").unwrap();
            let start = info.data_start().unwrap() as usize;
            (start, start + info.compressed_size() as usize)
        };
        let size = bytes.len();
        let server = TestServer::new(bytes).await;
        let metadata =
            RepodataFileMetadata::new(&server.op, "test.conda", PreconditionChecks::Enabled)
                .await
                .unwrap();
        let actual = timeout(
            Duration::from_secs(5),
            read_package(&server.op, "test.conda", &metadata),
        )
        .await
        .unwrap()
        .unwrap();
        assert_records_match(&actual, &expected);
        // A parser seek must not turn into another payload-sized HTTP response.
        let reads = server.reads.lock().unwrap();
        assert_eq!((reads[0].start, reads[0].end), (0, size));
        assert!(
            reads.len() <= 1 + MAX_METADATA_GETS,
            "excessive serial GETs: {reads:?}"
        );
        for read in &reads[1..] {
            let range = read.range.as_deref().expect("metadata GET lacked a range");
            assert!(!range.ends_with('-'), "unbounded metadata request: {range}");
            assert!(read.end - read.start <= METADATA_RESPONSE_LIMIT);
            assert_eq!(read.sent.load(Ordering::SeqCst), read.end - read.start);
        }
        // Metadata and digests are only interchangeable if every GET guards the same object.
        assert!(
            reads
                .iter()
                .all(|read| read.if_match.as_deref() == Some("\"version-1\""))
        );
        assert_eq!(
            (reads.last().unwrap().start, reads.last().unwrap().end),
            (info_start, info_end)
        );
        let transferred = reads
            .iter()
            .map(|read| read.sent.load(Ordering::SeqCst))
            .sum::<usize>();
        assert!(transferred <= size + MAX_METADATA_GETS * METADATA_RESPONSE_LIMIT);
    }
}
