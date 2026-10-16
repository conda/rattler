use std::net::SocketAddr;
use std::path::Path;

use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::Response;
use tower_http::services::ServeDir;
use url::Url;

/// Spawn a local file server with range-request support on a random port.
///
/// Returns the URL to the file (e.g. `http://127.0.0.1:12345/file.conda`).
pub async fn serve_file(file_path: impl AsRef<Path>) -> Url {
    let file_path = file_path.as_ref();
    let file_size = std::fs::metadata(file_path).unwrap().len();
    serve(file_path, |router| {
        router.layer(middleware::from_fn_with_state(
            file_size,
            clamp_suffix_range,
        ))
    })
    .await
}

/// Spawn a local file server that does NOT support range requests: incoming
/// `Range` headers are stripped, so every response is a full `200 OK`.
pub async fn serve_file_no_ranges(file_path: impl AsRef<Path>) -> Url {
    serve(file_path.as_ref(), |router| {
        router.layer(middleware::from_fn(strip_range))
    })
    .await
}

/// Spawn a local file server that answers any suffix range (`bytes=-N`) with
/// `416 Range Not Satisfiable`, mimicking `JFrog` Artifactory when the range
/// exceeds the object length.
pub async fn serve_file_416_suffix(file_path: impl AsRef<Path>) -> Url {
    serve(file_path.as_ref(), |router| {
        router.layer(middleware::from_fn(reject_suffix_range))
    })
    .await
}

/// Serves the directory containing `file_path` with the given middleware
/// applied, returning the URL of the file.
async fn serve(file_path: &Path, layer: impl FnOnce(axum::Router) -> axum::Router) -> Url {
    let file_name = file_path.file_name().unwrap().to_string_lossy().to_string();
    let dir = file_path.parent().unwrap();
    let app = layer(axum::Router::new().fallback_service(ServeDir::new(dir)));

    let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://{}:{}/{file_name}", addr.ip(), addr.port())
        .parse()
        .unwrap()
}

async fn reject_suffix_range(req: Request, next: Next) -> Response {
    let is_suffix = req
        .headers()
        .get(http::header::RANGE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|range| range.starts_with("bytes=-"));
    if is_suffix {
        return Response::builder()
            .status(http::StatusCode::RANGE_NOT_SATISFIABLE)
            .body(axum::body::Body::empty())
            .unwrap();
    }
    next.run(req).await
}

async fn strip_range(mut req: Request, next: Next) -> Response {
    req.headers_mut().remove(http::header::RANGE);
    let mut response = next.run(req).await;
    response.headers_mut().remove(http::header::ACCEPT_RANGES);
    response
}

/// Clamp suffix ranges (`bytes=-N`) that exceed the file size so `ServeDir`
/// doesn't return 416. Per RFC 9110 §14.1.2, a suffix range exceeding the
/// representation length should select the entire representation.
async fn clamp_suffix_range(
    State(file_size): State<u64>,
    mut req: Request,
    next: Next,
) -> Response {
    if let Some(range_val) = req.headers().get(http::header::RANGE)
        && let Ok(range_str) = range_val.to_str()
        && let Some(suffix) = range_str.strip_prefix("bytes=-")
        && let Ok(n) = suffix.parse::<u64>()
        && n > file_size
    {
        req.headers_mut().insert(
            http::header::RANGE,
            format!("bytes=0-{}", file_size - 1).parse().unwrap(),
        );
    }
    next.run(req).await
}

/// What a [`serve_file_observed`] server saw.
#[derive(Default)]
pub struct Observed {
    /// Requests currently being answered.
    in_flight: std::sync::atomic::AtomicUsize,
    /// The most requests answered at once.
    pub max_in_flight: std::sync::atomic::AtomicUsize,
    /// Requests for ranges after the first currently being answered.
    later_parts_in_flight: std::sync::atomic::AtomicUsize,
    /// The most requests for ranges after the first answered at once.
    pub max_later_parts_in_flight: std::sync::atomic::AtomicUsize,
    /// The first byte of each range requested, in the order answered.
    pub finished: std::sync::Mutex<Vec<u64>>,
}

/// Spawn a local file server with range-request support that holds every
/// response for `delay`, or for `slow_delay` when the range starts at
/// `slow_start`, so that requests overlap and finish out of order.
pub async fn serve_file_observed(
    file_path: impl AsRef<Path>,
    delay: std::time::Duration,
    slow_start: u64,
    slow_delay: std::time::Duration,
) -> (Url, std::sync::Arc<Observed>) {
    let observed = std::sync::Arc::new(Observed::default());
    let state = (observed.clone(), delay, slow_start, slow_delay);
    let url = serve(file_path.as_ref(), |router| {
        router.layer(middleware::from_fn_with_state(state, observe))
    })
    .await;
    (url, observed)
}

async fn observe(
    State((observed, delay, slow_start, slow_delay)): State<(
        std::sync::Arc<Observed>,
        std::time::Duration,
        u64,
        std::time::Duration,
    )>,
    req: Request,
    next: Next,
) -> Response {
    use std::sync::atomic::Ordering;

    let start = range_start(&req);
    let in_flight = observed.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    observed
        .max_in_flight
        .fetch_max(in_flight, Ordering::SeqCst);
    let later_part = start.is_some_and(|start| start > 0);
    if later_part {
        let in_flight = observed
            .later_parts_in_flight
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        observed
            .max_later_parts_in_flight
            .fetch_max(in_flight, Ordering::SeqCst);
    }
    let delay = if start == Some(slow_start) {
        slow_delay
    } else {
        delay
    };
    tokio::time::sleep(delay).await;
    let response = next.run(req).await;
    observed.in_flight.fetch_sub(1, Ordering::SeqCst);
    if later_part {
        observed
            .later_parts_in_flight
            .fetch_sub(1, Ordering::SeqCst);
    }
    observed.finished.lock().unwrap().extend(start);
    response
}

/// Spawn a local file server that honors a `Range` header on the first
/// request only, and answers every later one with the whole file.
pub async fn serve_file_ranges_once(file_path: impl AsRef<Path>) -> Url {
    let served = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    serve(file_path.as_ref(), |router| {
        router.layer(middleware::from_fn_with_state(served, range_once))
    })
    .await
}

async fn range_once(
    State(served): State<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    req: Request,
    next: Next,
) -> Response {
    if served.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return strip_range(req, next).await;
    }
    next.run(req).await
}

fn range_start(req: &Request) -> Option<u64> {
    req.headers()
        .get(http::header::RANGE)?
        .to_str()
        .ok()?
        .strip_prefix("bytes=")?
        .split_once('-')?
        .0
        .parse()
        .ok()
}

/// How a [`serve_file_overwritten`] server behaves.
#[derive(Clone, Copy)]
pub struct ObjectStore {
    /// Whether responses carry the headers that identify S3.
    pub s3: bool,
    /// Whether a request whose `If-Match` no longer matches gets a 412.
    pub honors_if_match: bool,
}

/// Spawn a local file server whose file is replaced by one with another entity
/// tag once the first request has been answered. The bytes served stay the
/// same, so only the tag reveals the change.
pub async fn serve_file_overwritten(file_path: impl AsRef<Path>, store: ObjectStore) -> Url {
    serve_etagged(file_path.as_ref(), store, true).await.0
}

/// Spawn a local file server that answers like S3, with an entity tag that
/// never changes and `If-Match` enforced. Also returns how many requests
/// carried a matching `If-Match`.
pub async fn serve_file_from_s3(
    file_path: impl AsRef<Path>,
) -> (Url, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let store = ObjectStore {
        s3: true,
        honors_if_match: true,
    };
    serve_etagged(file_path.as_ref(), store, false).await
}

#[derive(Clone)]
struct EtaggedObject {
    store: ObjectStore,
    overwritten: bool,
    requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    matched: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

async fn serve_etagged(
    file_path: &Path,
    store: ObjectStore,
    overwritten: bool,
) -> (Url, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let object = EtaggedObject {
        store,
        overwritten,
        requests: std::sync::Arc::default(),
        matched: std::sync::Arc::default(),
    };
    let matched = object.matched.clone();
    let url = serve(file_path, |router| {
        router.layer(middleware::from_fn_with_state(object, etag_object))
    })
    .await;
    (url, matched)
}

async fn etag_object(
    State(object): State<EtaggedObject>,
    mut req: Request,
    next: Next,
) -> Response {
    use std::sync::atomic::Ordering;

    let first = object.requests.fetch_add(1, Ordering::SeqCst) == 0;
    let etag = if first || !object.overwritten {
        "\"v1\""
    } else {
        "\"v2\""
    };
    // This server checks `If-Match` against its own ETag, not `ServeDir`.
    let if_match = req.headers_mut().remove(http::header::IF_MATCH);
    if if_match.as_ref().is_some_and(|value| value == etag) {
        object.matched.fetch_add(1, Ordering::SeqCst);
    }
    let mut response =
        if object.store.honors_if_match && if_match.is_some_and(|value| value != etag) {
            Response::builder()
                .status(http::StatusCode::PRECONDITION_FAILED)
                .body(axum::body::Body::empty())
                .unwrap()
        } else {
            next.run(req).await
        };
    let headers = response.headers_mut();
    headers.insert(http::header::ETAG, etag.parse().unwrap());
    if object.store.s3 {
        headers.insert("x-amz-request-id", "test".parse().unwrap());
    }
    response
}

/// How a [`serve_file_breaking_part`] server breaks one part.
#[derive(Clone, Copy)]
pub enum Breakage {
    /// The connection drops partway through the first answer.
    CutOnce,
    /// The body holds one byte more than the range.
    Overlong,
}

/// Spawn a local file server with range-request support that breaks the
/// answers to ranges starting at `start`. Also returns how many such ranges
/// were requested.
pub async fn serve_file_breaking_part(
    file_path: impl AsRef<Path>,
    start: u64,
    breakage: Breakage,
) -> (Url, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let state = (requests.clone(), start, breakage);
    let url = serve(file_path.as_ref(), |router| {
        router.layer(middleware::from_fn_with_state(state, break_part))
    })
    .await;
    (url, requests)
}

async fn break_part(
    State((requests, start, breakage)): State<(
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        u64,
        Breakage,
    )>,
    req: Request,
    next: Next,
) -> Response {
    if range_start(&req) != Some(start) {
        return next.run(req).await;
    }
    let first = requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
    let (mut parts, body) = next.run(req).await.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    parts.headers.remove(http::header::CONTENT_LENGTH);
    let body = match breakage {
        Breakage::CutOnce if first => {
            use futures::StreamExt;
            let half = bytes.slice(..bytes.len() / 2);
            // The drop comes after the headers, as a body read error.
            let drop = async {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                Err(std::io::Error::other("connection dropped"))
            };
            axum::body::Body::from_stream(
                futures::stream::once(async { Ok(half) }).chain(futures::stream::once(drop)),
            )
        }
        Breakage::CutOnce => axum::body::Body::from(bytes),
        Breakage::Overlong => axum::body::Body::from([&bytes[..], b"x"].concat()),
    };
    Response::from_parts(parts, body)
}

/// Spawn a local file server that answers at most `max_len` bytes of any
/// range, like a server or proxy with its own part size.
pub async fn serve_file_capping_ranges(file_path: impl AsRef<Path>, max_len: u64) -> Url {
    serve(file_path.as_ref(), |router| {
        router.layer(middleware::from_fn_with_state(max_len, cap_range))
    })
    .await
}

async fn cap_range(State(max_len): State<u64>, mut req: Request, next: Next) -> Response {
    if let Some(start) = range_start(&req) {
        let range = format!("bytes={start}-{}", start + max_len - 1);
        req.headers_mut()
            .insert(http::header::RANGE, range.parse().unwrap());
    }
    next.run(req).await
}
