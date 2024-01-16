use axum::routing::get_service;
use reqwest::Url;
use std::net::SocketAddr;
use std::path::Path;
use tokio::sync::oneshot;
use tower_http::services::ServeDir;

/// A convenient async HTTP server that serves the content of a folder. The server only listens to
/// `127.0.0.1` and uses a random port. This makes it safe to run multiple instances. Its perfect to
/// use for testing HTTP file requests.
pub struct StaticDirectoryServer {
    local_addr: SocketAddr,
    shutdown_sender: Option<oneshot::Sender<()>>,
}

impl StaticDirectoryServer {
    /// Returns the root `Url` to the server.
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://localhost:{}", self.local_addr.port())).unwrap()
    }
}

impl StaticDirectoryServer {
    pub fn new(path: impl AsRef<Path>) -> Self {
        let service = get_service(ServeDir::new(path));

        // Create a router that will serve the static files
        let app = axum::Router::new().nest_service("/", service);

        // Construct the server that will listen on localhost but with a *random port*. The random
        // port is very important because it enables creating multiple instances at the same time.
        // We need this to be able to run tests in parallel.
        let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
        let server = axum::Server::bind(&addr).serve(app.into_make_service());

        // Get the address of the server so we can bind to it at a later stage.
        let addr = server.local_addr();

        // Setup a graceful shutdown trigger which is fired when this instance is dropped.
        let (tx, rx) = oneshot::channel();
        let server = server.with_graceful_shutdown(async {
            rx.await.ok();
        });

        // Spawn the server. Let go of the JoinHandle, we can use the graceful shutdown trigger to
        // stop the server.
        tokio::spawn(server);

        Self {
            local_addr: addr,
            shutdown_sender: Some(tx),
        }
    }
}

impl Drop for StaticDirectoryServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_sender.take() {
            let _ = tx.send(());
        }
    }
}
