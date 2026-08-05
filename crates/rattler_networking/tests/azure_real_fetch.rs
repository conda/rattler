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
//! AZURE_TEST_ACCOUNT=stcondachannel \
//! AZURE_TEST_CONTAINER=general \
//! AZURE_TEST_PATH=noarch/repodata.json \
//! cargo test -p rattler_networking --features azure --test azure_real_fetch -- --ignored --nocapture
//! ```
//!
//! Set `AZURE_TEST_HOST` to target a sovereign cloud or emulator directly
//! (overrides the default `{account}.blob.core.windows.net` host).

use std::collections::HashMap;

use rattler_azure::{Auth, AzureFetchOptions, AzureHost};
use rattler_networking::AzureMiddleware;
use reqwest_middleware::ClientBuilder;

#[tokio::test]
#[ignore = "requires az login and AZURE_TEST_* env vars pointing at a real account"]
async fn azure_middleware_fetches_real_repodata() {
    let account = std::env::var("AZURE_TEST_ACCOUNT").expect("AZURE_TEST_ACCOUNT");
    let container = std::env::var("AZURE_TEST_CONTAINER").expect("AZURE_TEST_CONTAINER");
    let path =
        std::env::var("AZURE_TEST_PATH").unwrap_or_else(|_| "noarch/repodata.json".to_string());
    let host = std::env::var("AZURE_TEST_HOST")
        .unwrap_or_else(|_| format!("{account}.blob.core.windows.net"));

    // The grant is what makes this the AAD test rather than an anonymous read: an
    // `azure-options` entry for the host is the only thing that lets the
    // `az login` credential attach to it, and it makes a broken credential fail
    // loudly instead of falling through to an unsigned 404.
    let options = HashMap::from([(
        AzureHost::parse(&host).expect("AZURE_TEST_HOST is not a valid host[:port]"),
        AzureFetchOptions {
            auth: Auth::DefaultChain,
            ..Default::default()
        },
    )]);

    let client = ClientBuilder::new(reqwest::Client::new())
        .with(AzureMiddleware::new(reqwest::Client::new(), options))
        .build();

    // The `az://` host carries the full blob endpoint — same form used in a
    // channel URL, e.g. `az://stcondachannel.blob.core.windows.net/general`.
    let url = format!("az://{host}/{container}/{path}");
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
