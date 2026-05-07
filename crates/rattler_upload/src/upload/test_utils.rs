use std::net::SocketAddr;
use std::path::PathBuf;

use url::Url;

pub(crate) async fn start_test_server(router: axum::Router) -> Url {
    let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{}:{}", addr.ip(), addr.port())
        .parse()
        .unwrap()
}

pub(crate) fn test_package_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data/packages/empty-0.1.0-h4616a5c_0.conda")
}
