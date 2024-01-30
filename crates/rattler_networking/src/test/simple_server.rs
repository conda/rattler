use std::future::IntoFuture;

use axum::{http::HeaderMap, response::IntoResponse, routing::get, Router};

async fn health_checker_handler() -> impl IntoResponse {
    return "test";
}

async fn simple_http_auth(headers: HeaderMap) {
    println!("Headers: {:?}", headers);
}

struct SimpleServer {
    shutdown_sender: Option<oneshot::Sender<()>>,
    address: String,
}

async fn spawn() -> SimpleServer {
    let app = Router::new()
        .route("/api/healthchecker", get(health_checker_handler))
        .route("/api/http_auth", get(simple_http_auth));
    let address = "0.0.0.0:1234".to_string();
    let listener = tokio::net::TcpListener::bind(&address).await.unwrap();
    let (tx, rx) = oneshot::channel();

    let server = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            rx.await.ok();
        })
        .into_future();

    tokio::spawn(server);

    println!("🚀 Server started successfully");

    SimpleServer {
        shutdown_sender: Some(tx),
        address,
    }
}

impl SimpleServer {
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.address, path)
    }
}

impl Drop for SimpleServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_sender.take() {
            let _ = tx.send(());
        }
    }
}

#[tokio::test]
async fn test_server() {
    let server = spawn().await;

    let client = reqwest::Client::new();
    let response = client
        .get(server.url("/api/http_auth"))
        .header("bla", "piep")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "test");
}
