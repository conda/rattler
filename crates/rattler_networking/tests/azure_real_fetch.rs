//! Live read-path test against a *real* Azure Blob Storage account.
//!
//! Exercises the AAD / token path of reqsign's `DefaultCredentialProvider` —
//! i.e. the credential `az login` provides — against a real
//! `*.blob.core.windows.net` endpoint. It is purely a read; it never writes.
//!
//! Configure via env and run (requires `az login` with at least
//! `Storage Blob Data Reader` on the container):
//!
//! ```text
//! AZURE_TEST_ACCOUNT=stgrcondachannel \
//! AZURE_TEST_CONTAINER=general \
//! AZURE_TEST_PATH=noarch/repodata.json \
//! cargo test -p rattler_networking --features azure --test azure_real_fetch -- --ignored --nocapture
//! ```
#![cfg(feature = "azure")]

use std::collections::HashMap;

use rattler_networking::{AzureMiddleware, azure_middleware::AzureConfig};
use reqwest_middleware::ClientBuilder;
use url::Url;

#[tokio::test]
#[ignore = "requires az login and AZURE_TEST_* env vars pointing at a real account"]
async fn azure_middleware_fetches_real_repodata() {
    let account = std::env::var("AZURE_TEST_ACCOUNT").expect("AZURE_TEST_ACCOUNT");
    let container = std::env::var("AZURE_TEST_CONTAINER").expect("AZURE_TEST_CONTAINER");
    let path =
        std::env::var("AZURE_TEST_PATH").unwrap_or_else(|_| "noarch/repodata.json".to_string());

    // An explicit endpoint override addresses sovereign clouds / emulators; the
    // default (account form) targets `https://{account}.blob.core.windows.net`.
    let azure_config = match std::env::var("AZURE_TEST_ENDPOINT").ok() {
        Some(endpoint) => {
            AzureConfig::Endpoint(Url::parse(&endpoint).expect("invalid AZURE_TEST_ENDPOINT"))
        }
        None => AzureConfig::Account(account),
    };

    let mut config = HashMap::new();
    config.insert(container.clone(), azure_config);

    let client = ClientBuilder::new(reqwest::Client::new())
        .with(AzureMiddleware::new(config))
        .build();

    let url = format!("az://{container}/{path}");
    println!("fetching {url}");
    let resp = client
        .get(&url)
        .send()
        .await
        .expect("request through azure middleware failed");

    let status = resp.status();
    let body = resp.bytes().await.expect("failed to read body");
    println!("status={status} bytes={}", body.len());
    assert!(status.is_success(), "unexpected status {status}: {url}");

    let json: serde_json::Value =
        serde_json::from_slice(&body).expect("real repodata was not valid json");
    assert!(
        json.get("info").is_some() || json.get("packages").is_some(),
        "fetched body does not look like repodata: {json}"
    );
    println!("info = {}", json["info"]);
}
