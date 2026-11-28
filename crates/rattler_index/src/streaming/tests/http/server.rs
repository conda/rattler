//! Serve conditional, byte-counted ranges for the S3 client's HTTP regression test.

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Method, Request, Response, StatusCode, header},
    serve as serve_http,
};
use bytes::Bytes;
use futures::stream;
use opendal::{BytesRange, Operator};
use rattler_s3::{ResolvedS3Credentials, S3AddressingStyle, S3CredentialSource};
use std::{
    convert::Infallible,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{net::TcpListener, task::JoinSet};

#[derive(Clone, Debug)]
pub(super) struct Read {
    pub(super) range: Option<String>,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) if_match: Option<String>,
    pub(super) sent: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct Archive {
    bytes: Bytes,
    reads: Arc<Mutex<Vec<Read>>>,
}

/// Count bytes actually emitted, not only requested ranges, to catch read-ahead that
/// would silently defeat the memory and transfer savings. A fixed `ETag` makes every
/// request condition observable without needing credentials or an external S3 server.
async fn serve(State(state): State<Archive>, request: Request<Body>) -> Response<Body> {
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
    let body = Body::from_stream(stream::iter((start..end).step_by(16 * 1024).map(
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

/// Keep the server alive until the test drops its request log and operator.
pub(super) struct TestServer {
    pub(super) op: Operator,
    pub(super) reads: Arc<Mutex<Vec<Read>>>,
    _tasks: JoinSet<()>,
}

impl TestServer {
    /// Exercise the real S3 reader against a local archive with a fixed object identity.
    pub(super) async fn new(bytes: Vec<u8>) -> Self {
        let reads = Arc::new(Mutex::new(Vec::new()));
        let state = Archive {
            bytes: Bytes::from(bytes),
            reads: reads.clone(),
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let mut tasks = JoinSet::new();
        tasks.spawn(async move {
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
        Self {
            op,
            reads,
            _tasks: tasks,
        }
    }
}
