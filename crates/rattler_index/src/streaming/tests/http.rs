//! Exercise `OpenDAL`'s actual S3 HTTP requests: a seekable in-memory backend cannot
//! reveal offset-to-EOF downloads, missing `If-Match` headers, or excessive range requests.

use super::*;
use axum::serve as serve_http;
use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Method, Request, Response, StatusCode, header},
};
use bytes::Bytes;
use opendal::BytesRange;
use rattler_s3::{ResolvedS3Credentials, S3AddressingStyle, S3CredentialSource};
use std::{
    convert::Infallible,
    sync::{Mutex, atomic::AtomicUsize},
};
use tokio::{net::TcpListener, task::JoinSet};

#[derive(Clone, Debug)]
struct Read {
    range: Option<String>,
    start: usize,
    end: usize,
    if_match: Option<String>,
    sent: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct Server {
    bytes: Bytes,
    reads: Arc<Mutex<Vec<Read>>>,
}

/// Count bytes actually emitted, not only requested ranges, to catch read-ahead that
/// would silently defeat the memory and transfer savings. A fixed `ETag` makes every
/// request condition observable without needing credentials or an external S3 server.
async fn serve(State(state): State<Server>, request: Request<Body>) -> Response<Body> {
    let length = state.bytes.len();
    if request.method() == Method::HEAD {
        return Response::builder()
            .header(header::CONTENT_LENGTH, length)
            .header(header::ETAG, "\"version-1\"")
            .body(Body::empty())
            .unwrap();
    }
    assert_eq!(request.method(), Method::GET);
    let if_match = request
        .headers()
        .get(header::IF_MATCH)
        .map(|v| v.to_str().unwrap().to_owned());
    if if_match
        .as_deref()
        .is_some_and(|value| value != "\"version-1\"")
    {
        return Response::builder()
            .status(StatusCode::PRECONDITION_FAILED)
            .body(Body::empty())
            .unwrap();
    }
    let range = request
        .headers()
        .get(header::RANGE)
        .map(|v| v.to_str().unwrap().to_owned());
    let requested = range
        .as_deref()
        .unwrap_or("bytes=0-")
        .parse::<BytesRange>()
        .unwrap()
        .to_content_range(length)
        .unwrap();
    let (start, end) = (requested.start, requested.end);
    assert!(start < end && end <= length);
    let sent = Arc::new(AtomicUsize::new(0));
    state.reads.lock().unwrap().push(Read {
        range: range.clone(),
        start,
        end,
        if_match,
        sent: sent.clone(),
    });
    let body = Body::from_stream(futures::stream::iter((start..end).step_by(16 * 1024).map(
        move |offset| {
            let chunk = state.bytes.slice(offset..(offset + 16 * 1024).min(end));
            sent.fetch_add(chunk.len(), Ordering::SeqCst);
            Ok::<_, Infallible>(chunk)
        },
    )));
    let mut response = Response::builder()
        .header(header::CONTENT_LENGTH, end - start)
        .header(header::ETAG, "\"version-1\"");
    if range.is_some() {
        response = response.status(StatusCode::PARTIAL_CONTENT).header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{}/{length}", end - 1),
        );
    }
    response.body(body).unwrap()
}

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
        let reads = Arc::new(Mutex::new(Vec::new()));
        let state = Server {
            bytes: Bytes::from(bytes),
            reads: reads.clone(),
        };
        let size = state.bytes.len();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let mut server = JoinSet::new();
        server.spawn(async move {
            serve_http(listener, Router::new().fallback(serve).with_state(state))
                .await
                .unwrap();
        });
        let credentials = ResolvedS3Credentials {
            endpoint_url: endpoint.parse().unwrap(),
            region: "us-east-1".to_owned(),
            addressing_style: S3AddressingStyle::Path,
            access_key_id: "test".to_owned(),
            secret_access_key: "test".to_owned(),
            session_token: None,
        };
        let op = S3CredentialSource::from(credentials)
            .opendal_operator("test", "")
            .unwrap();
        let metadata = RepodataFileMetadata::new(&op, "test.conda", PreconditionChecks::Enabled)
            .await
            .unwrap();
        let actual = timeout(
            Duration::from_secs(5),
            read_package(&op, "test.conda", &metadata),
        )
        .await
        .unwrap()
        .unwrap();
        assert_records_match(&actual, &expected);
        // A parser seek must not turn into another payload-sized HTTP response.
        let reads = reads.lock().unwrap();
        assert_eq!((reads[0].start, reads[0].end), (0, size));
        assert!(reads.len() <= 12, "excessive serial GETs: {reads:?}");
        for read in &reads[1..] {
            let range = read.range.as_deref().expect("metadata GET lacked a range");
            assert!(!range.ends_with('-'), "unbounded metadata request: {range}");
            assert!(read.end - read.start <= 64 * 1024);
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
        assert!(transferred <= size + 1024 * 1024);
        eprintln!(
            "S3_RANGE_PROFILE info_first={info_first} archive_bytes={size} gets={} transferred_bytes={transferred} metadata_overhead_bytes={}",
            reads.len(),
            transferred - size
        );
        drop(server);
    }
}
