use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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

/// Like [`serve_file`], but also counts the response body bytes the server
/// hands to the transport. Aborting a request on the client side stops the
/// count (up to what already sits in socket buffers).
pub async fn serve_file_counting(file_path: impl AsRef<Path>) -> (Url, Arc<AtomicU64>) {
    let file_path = file_path.as_ref();
    let file_size = std::fs::metadata(file_path).unwrap().len();
    let sent = Arc::new(AtomicU64::new(0));
    let counter = sent.clone();
    let url = serve(file_path, move |router| {
        router
            .layer(middleware::from_fn_with_state(counter, count_body_bytes))
            .layer(middleware::from_fn_with_state(
                file_size,
                clamp_suffix_range,
            ))
    })
    .await;
    (url, sent)
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

async fn count_body_bytes(
    State(sent): State<Arc<AtomicU64>>,
    req: Request,
    next: Next,
) -> Response {
    use futures_util::StreamExt;
    let response = next.run(req).await;
    response.map(|body| {
        axum::body::Body::from_stream(body.into_data_stream().inspect(move |chunk| {
            if let Ok(chunk) = chunk {
                sent.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            }
        }))
    })
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
