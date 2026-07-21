//! Probe: does opendal's azblob backend populate `content_md5` on list?
//!
//! This is exactly the signal the md5-based re-index check in `index_subdir_inner`
//! relies on (`entry.metadata().content_md5()`). If azure doesn't set it on a plain
//! `list_with`, that check silently forces a full re-index of the channel.
//!
//! Run with:
//! ```text
//! docker run -p 10000:10000 mcr.microsoft.com/azure-storage/azurite \
//!     azurite-blob --blobHost 0.0.0.0 --skipApiVersionCheck
//! AZURE_STORAGE_ACCOUNT_KEY=Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw== \
//! cargo test -p rattler_index --features azure --test azure_md5_probe -- --ignored --nocapture
//! ```
#![cfg(feature = "azure")]

use base64::Engine;
use opendal::{Configurator, Operator, services::AzblobConfig};

const ACCOUNT: &str = "devstoreaccount1";
const CONTAINER: &str = "md5-probe";
const ENDPOINT: &str = "http://127.0.0.1:10000/devstoreaccount1";

fn azurite_operator() -> Operator {
    let key = std::env::var("AZURE_STORAGE_ACCOUNT_KEY").expect("AZURE_STORAGE_ACCOUNT_KEY");
    let cfg = AzblobConfig {
        container: CONTAINER.to_string(),
        account_name: Some(ACCOUNT.to_string()),
        account_key: Some(key),
        endpoint: Some(ENDPOINT.to_string()),
        ..Default::default()
    };
    Operator::new(cfg.into_builder()).unwrap().finish()
}

#[tokio::test]
#[ignore = "requires a running Azurite emulator"]
async fn azure_populates_content_md5_on_list() {
    let op = azurite_operator();
    let _ = op.create_dir("noarch/").await;

    let name = "noarch/empty-0.1.0-h4616a5c_0.conda";
    let bytes = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/packages/empty-0.1.0-h4616a5c_0.conda"),
    )
    .unwrap();
    op.write(name, bytes).await.unwrap();

    // Exactly what index_subdir_inner does: plain list, then read content_md5 off
    // each entry's listing metadata.
    let entries = op.list_with("noarch/").await.unwrap();
    let mut saw_file = false;
    for entry in &entries {
        let meta = entry.metadata();
        if !meta.mode().is_file() {
            continue;
        }
        saw_file = true;
        println!(
            "LIST  {} -> content_md5 = {:?}",
            entry.name(),
            meta.content_md5()
        );
    }
    assert!(saw_file, "no file entry found in listing");

    // For contrast: does an explicit stat populate it?
    let stat = op.stat(name).await.unwrap();
    println!("STAT  {name} -> content_md5 = {:?}", stat.content_md5());

    // The re-index check decodes this base64 header to compare against the record's
    // raw 16-byte md5, so lock in that it is standard-base64 of exactly 16 bytes.
    let md5 = stat
        .content_md5()
        .expect("azure did not populate content_md5");
    let raw = base64::engine::general_purpose::STANDARD
        .decode(md5)
        .expect("content_md5 was not valid base64");
    assert_eq!(raw.len(), 16, "decoded md5 was not 16 bytes: {raw:?}");
}
