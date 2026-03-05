use std::net::SocketAddr;
use std::path::Path;

use axum::extract::Request;
use axum::response::IntoResponse;
use http::StatusCode;
use url::Url;

/// Spawn a local file server with range-request support on a random port.
///
/// Returns the URL to the file (e.g. `http://127.0.0.1:12345/file.conda`).
///
/// We need a custom handler instead of `tower_http::ServeDir` because
/// `ServeDir` returns 416 for suffix ranges (`bytes=-N`) that exceed the file
/// size, which is exactly what `async_http_range_reader` sends on its initial
/// probe for small files.
pub async fn serve_file(file_path: impl AsRef<Path>) -> Url {
    let file_path = file_path.as_ref();
    let file_name = file_path.file_name().unwrap().to_string_lossy().to_string();
    let data = std::fs::read(file_path).expect("failed to read test file");

    let app = axum::Router::new().fallback(move |req: Request| {
        let data = data.clone();
        async move { serve_with_ranges(&data, req.headers()) }
    });

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

fn serve_with_ranges(data: &[u8], headers: &http::HeaderMap) -> impl IntoResponse {
    let total = data.len();

    if let Some(range_header) = headers.get(http::header::RANGE) {
        let range_str = range_header.to_str().unwrap_or("");
        if let Some(spec) = range_str.strip_prefix("bytes=") {
            if let Some(neg) = spec.strip_prefix('-') {
                // Suffix range: bytes=-N → last N bytes (clamped to file size)
                let n: usize = neg.parse().unwrap_or(0);
                let start = total.saturating_sub(n);
                return partial(data, start, total - 1, total);
            } else if let Some((s, e)) = spec.split_once('-') {
                let start: usize = s.parse().unwrap_or(0);
                let end: usize = if e.is_empty() {
                    total - 1
                } else {
                    e.parse().unwrap_or(total - 1).min(total - 1)
                };
                return partial(data, start, end, total);
            }
        }
    }

    (
        StatusCode::OK,
        [
            (http::header::CONTENT_LENGTH, total.to_string()),
            (http::header::ACCEPT_RANGES, "bytes".to_string()),
        ],
        data.to_vec(),
    )
        .into_response()
}

fn partial(data: &[u8], start: usize, end: usize, total: usize) -> axum::response::Response {
    let slice = &data[start..=end];
    (
        StatusCode::PARTIAL_CONTENT,
        [
            (
                http::header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{total}"),
            ),
            (http::header::CONTENT_LENGTH, slice.len().to_string()),
            (http::header::ACCEPT_RANGES, "bytes".to_string()),
        ],
        slice.to_vec(),
    )
        .into_response()
}
