use std::{
    time::Duration,
};

use rattler_package_streaming::{reqwest::tokio::extract_tar_bz2, ExtractError, ExtractResult};
use reqwest::Client;
use reqwest_middleware::ClientWithMiddleware;
use tempfile::TempDir;
use url::Url;


use criterion::{criterion_group, criterion_main, Criterion};

enum TarUrls{
    Python,
    Bat,
    Boltons
}

impl TarUrls {
    fn get_url(&self) -> Url {
        match self {
            TarUrls::Python => Url::parse("http://localhost:8008/python").unwrap(),
            TarUrls::Bat => Url::parse("http://localhost:8008/bat").unwrap(),
            TarUrls::Boltons => Url::parse("http://localhost:8008/boltons").unwrap(),
        }
    }
}

async fn extract_tar(url: TarUrls) -> Result<ExtractResult, ExtractError> {
    // Create a new temporary directory for this run
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let temp_path = temp_dir.path();

    // Build a client without connection reuse
    let client = Client::builder()
        .pool_max_idle_per_host(0) // Disable connection pooling
        .build()
        .expect("Failed to build reqwest client");

    extract_tar_bz2(
        ClientWithMiddleware::from(client),
        url.get_url(),
        temp_path,
        None,
        None,
    )
    .await
}

/// Before running the benchmark
/// you need to start the server by running the following command:
/// `pixi run run-server`
fn criterion_benchmark(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("extract tars", |b| {
        b.to_async(&rt)
            .iter(|| async { 
                extract_tar(TarUrls::Bat).await.unwrap();
            });
    });
}

criterion_group!(
    name = benches;
    config = Criterion::default().measurement_time(Duration::from_secs(210)).sample_size(10);
    targets = criterion_benchmark
);
criterion_main!(benches);
