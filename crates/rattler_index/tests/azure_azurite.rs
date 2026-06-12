//! Integration test against a local Azurite emulator.
//!
//! Run with:
//!   docker run -p 10000:10000 mcr.microsoft.com/azure-storage/azurite \
//!       azurite-blob --blobHost 0.0.0.0
//!   AZURE_STORAGE_ACCOUNT_KEY=Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw== \
//!   cargo test -p rattler_index --features azure --test azure_azurite -- --ignored --nocapture
#![cfg(feature = "azure")]

use opendal::{services::AzblobConfig, Configurator, Operator};
use url::Url;

const ACCOUNT: &str = "devstoreaccount1";
const CONTAINER: &str = "test-channel";
const ENDPOINT: &str = "http://127.0.0.1:10000/devstoreaccount1";

fn azurite_operator(root: &str) -> Operator {
    let key = std::env::var("AZURE_STORAGE_ACCOUNT_KEY").expect("AZURE_STORAGE_ACCOUNT_KEY");
    let mut cfg = AzblobConfig::default();
    cfg.root = Some(root.to_string());
    cfg.container = CONTAINER.to_string();
    cfg.account_name = Some(ACCOUNT.to_string());
    cfg.account_key = Some(key);
    cfg.endpoint = Some(ENDPOINT.to_string());
    Operator::new(cfg.into_builder()).unwrap().finish()
}

fn test_package_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data/packages")
        .join(test_package_name())
}

fn test_package_name() -> &'static str {
    "empty-0.1.0-h4616a5c_0.conda"
}

#[tokio::test]
#[ignore = "requires a running Azurite emulator"]
async fn index_azure_creates_repodata() {
    let channel_root = format!("/{CONTAINER}-chan");
    let op = azurite_operator(&channel_root);
    let pkg_bytes = std::fs::read(test_package_path()).unwrap();
    op.write(&format!("noarch/{}", test_package_name()), pkg_bytes)
        .await
        .unwrap();

    let channel = Url::parse(&format!("az://{CONTAINER}{channel_root}")).unwrap();
    rattler_index::index_azure(rattler_index::IndexAzureConfig {
        channel: channel.clone(),
        account: ACCOUNT.to_string(),
        endpoint_url: Some(Url::parse(ENDPOINT).unwrap()),
        target_platform: None,
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        repodata_revisions: Vec::new(),
        package_revision_assignment: Default::default(),
        force: true,
        max_parallel: 4,
        multi_progress: None,
        precondition_checks: rattler_index::PreconditionChecks::Disabled,
    })
    .await
    .unwrap();

    let repodata = op.read("noarch/repodata.json").await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&repodata.to_vec()).unwrap();
    let has_packages = json["packages"]
        .as_object()
        .map(|m| !m.is_empty())
        .unwrap_or(false)
        || json["packages.conda"]
            .as_object()
            .map(|m| !m.is_empty())
            .unwrap_or(false);
    assert!(has_packages, "repodata should list the seeded package: {json}");
}
