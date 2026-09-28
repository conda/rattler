//! A local HTTP server that serves a directory with range-request support.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use axum::{
    extract::{Request, State},
    middleware::{self, Next},
    response::Response,
};
use tower_http::services::ServeDir;
use url::Url;

/// Serves `dir` on a random port and returns its base url.
pub async fn serve_dir(dir: &Path) -> Url {
    let root = dir.to_path_buf();
    serve(dir, |router| {
        router.layer(middleware::from_fn_with_state(root, clamp_range))
    })
    .await
}

/// Serves `dir`, answering suffix ranges (`bytes=-N`) with
/// `416 Range Not Satisfiable` like `JFrog` Artifactory does when the range
/// exceeds the object length.
pub async fn serve_dir_without_suffix_ranges(dir: &Path) -> Url {
    serve(dir, |router| {
        router.layer(middleware::from_fn(reject_suffix_range))
    })
    .await
}

async fn serve(dir: &Path, layer: impl FnOnce(axum::Router) -> axum::Router) -> Url {
    let app = layer(axum::Router::new().fallback_service(ServeDir::new(dir)));
    let listener = tokio::net::TcpListener::bind(&SocketAddr::new([127, 0, 0, 1].into(), 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{}:{}/", addr.ip(), addr.port())
        .parse()
        .unwrap()
}

async fn reject_suffix_range(req: Request, next: Next) -> Response {
    if is_suffix_range(&req) {
        return Response::builder()
            .status(http::StatusCode::RANGE_NOT_SATISFIABLE)
            .body(axum::body::Body::empty())
            .unwrap();
    }
    next.run(req).await
}

/// Clamps ranges that exceed the size of the file to the whole file: `ServeDir`
/// answers them with `416`, while RFC 9110 §14.1.2 asks for the whole
/// representation.
async fn clamp_range(State(root): State<PathBuf>, mut req: Request, next: Next) -> Response {
    let range = req
        .headers()
        .get(http::header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    if let Some(range) = range {
        let path = root.join(req.uri().path().trim_start_matches('/'));
        let size = std::fs::metadata(&path).map_or(0, |metadata| metadata.len());
        let too_large = range
            .strip_prefix("bytes=-")
            .and_then(|suffix| suffix.parse::<u64>().ok())
            .is_some_and(|last| last > size)
            || range
                .strip_prefix("bytes=0-")
                .and_then(|end| end.parse::<u64>().ok())
                .is_some_and(|end| size > 0 && end >= size);
        if too_large && size > 0 {
            req.headers_mut().insert(
                http::header::RANGE,
                format!("bytes=0-{}", size - 1).parse().unwrap(),
            );
        }
    }
    next.run(req).await
}

fn is_suffix_range(req: &Request) -> bool {
    req.headers()
        .get(http::header::RANGE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|range| range.starts_with("bytes=-"))
}
