#![allow(dead_code)]

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
    let file_name = file_path.file_name().unwrap().to_string_lossy().to_string();
    let dir = file_path.parent().unwrap();
    let file_size = std::fs::metadata(file_path).unwrap().len();

    let app = axum::Router::new()
        .fallback_service(ServeDir::new(dir))
        .layer(middleware::from_fn_with_state(
            file_size,
            clamp_suffix_range,
        ));

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

/// Clamp suffix ranges (`bytes=-N`) that exceed the file size so `ServeDir`
/// doesn't return 416. Per RFC 9110 §14.1.2, a suffix range exceeding the
/// representation length should select the entire representation.
async fn clamp_suffix_range(
    State(file_size): State<u64>,
    mut req: Request,
    next: Next,
) -> Response {
    if let Some(range_val) = req.headers().get(http::header::RANGE) {
        if let Ok(range_str) = range_val.to_str() {
            if let Some(suffix) = range_str.strip_prefix("bytes=-") {
                if let Ok(n) = suffix.parse::<u64>() {
                    if n > file_size {
                        req.headers_mut().insert(
                            http::header::RANGE,
                            format!("bytes=0-{}", file_size - 1).parse().unwrap(),
                        );
                    }
                }
            }
        }
    }
    next.run(req).await
}

/// Spawn a local file server for a directory
pub async fn serve_dir(dir: impl AsRef<Path>) -> Url {
    let dir = dir.as_ref();
    let app = axum::Router::new().fallback_service(ServeDir::new(dir));
    let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{}:{}/", addr.ip(), addr.port())
        .parse()
        .unwrap()
}
