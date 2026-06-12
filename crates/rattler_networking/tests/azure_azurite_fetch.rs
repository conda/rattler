//! Live read-path integration test against a local Azurite emulator.
//!
//! Exercises `AzureMiddleware`: it rewrites an `az://` URL to the Azurite
//! HTTPS-equivalent and signs it via reqsign's shared-key credential, then we
//! actually fetch a blob that was seeded into the emulator.
//!
//! Prereqs (mirrors the index-side test at `rattler_index/tests/azure_azurite.rs`):
//!
//! ```text
//! docker run -p 10000:10000 mcr.microsoft.com/azure-storage/azurite \
//!     azurite-blob --blobHost 0.0.0.0 --skipApiVersionCheck
//! # seed a `cli-channel` container with `noarch/repodata.json`
//! AZURE_STORAGE_ACCOUNT_NAME=devstoreaccount1 \
//! AZURE_STORAGE_ACCOUNT_KEY=Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw== \
//! cargo test -p rattler_networking --features azure --test azure_azurite_fetch -- --ignored --nocapture
//! ```
#![cfg(feature = "azure")]

use std::collections::HashMap;

use rattler_config::config::azure::AzureOptions;
use rattler_networking::AzureMiddleware;
use reqwest_middleware::ClientBuilder;
use url::Url;

const CONTAINER: &str = "cli-channel";
const ENDPOINT: &str = "http://127.0.0.1:10000/devstoreaccount1";

#[tokio::test]
#[ignore = "requires a running Azurite emulator seeded with cli-channel/noarch/repodata.json"]
async fn azure_middleware_fetches_repodata() {
    let mut config = HashMap::new();
    config.insert(
        CONTAINER.to_string(),
        AzureOptions {
            account: "devstoreaccount1".to_string(),
            endpoint_url: Some(Url::parse(ENDPOINT).unwrap()),
        },
    );

    let client = ClientBuilder::new(reqwest::Client::new())
        .with(AzureMiddleware::new(config))
        .build();

    let resp = client
        .get(format!("az://{CONTAINER}/noarch/repodata.json"))
        .send()
        .await
        .expect("request through azure middleware failed");

    assert!(
        resp.status().is_success(),
        "unexpected status {} fetching repodata",
        resp.status()
    );

    let json: serde_json::Value = resp.json().await.expect("repodata was not valid json");
    let has_pkg = json["packages.conda"]
        .as_object()
        .is_some_and(|m| !m.is_empty())
        || json["packages"].as_object().is_some_and(|m| !m.is_empty());
    assert!(
        has_pkg,
        "repodata fetched via az:// should list a package: {json}"
    );
}
