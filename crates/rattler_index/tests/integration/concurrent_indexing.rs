// Tests for concurrent indexing with race condition handling
//
// These tests use synchronization barriers to create deterministic race
// conditions:
// 1. Process A collects ETags, indexes packages
// 2. Process B collects ETags (same as A), indexes packages
// 3. Process A writes repodata (ETag changes)
// 4. Process B tries to write repodata but fails (ETag mismatch)
// 5. Process B retries from the beginning

use std::path::Path;

use opendal::Operator;
use rattler_conda_types::Platform;
use rattler_index::index;
use tracing::Instrument;

use super::etag_memory_backend::ETagMemoryBuilder;

#[tokio::test]
async fn test_concurrent_index_with_race_condition_and_retry() {
    // Initialize tracing subscriber to see log output
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .compact()
        .with_target(false)
        .try_init();

    // Create operator with ETag support
    let op = Operator::new(ETagMemoryBuilder::default())
        .unwrap()
        .finish();

    // Create subdirectories and copy files from filesystem to our ETag backend
    let contents = tokio::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/packages/empty-0.1.0-h4616a5c_0.conda"),
    )
    .await
    .unwrap();
    op.write("noarch/empty-0.1.0-h4616a5c_0.conda", contents)
        .await
        .unwrap();

    let op1 = op.clone();
    let op2 = op.clone();

    // Process 1: will complete first
    let handle1 = tokio::spawn(
        async move {
            tracing::info!("Starting index");
            let result = index(
                Some(Platform::NoArch),
                op1,
                None,
                false,
                false,
                false,
                1,
                None,
            )
            .await;
            tracing::info!("Finished - {:?}", result.is_ok());
            result
        }
        .instrument(tracing::info_span!("Process 1")),
    );

    // Process 2: will encounter race condition and retry
    let handle2 = tokio::spawn(
        async move {
            tracing::info!("Starting index");
            let result = index(
                Some(Platform::NoArch),
                op2,
                None,
                false,
                false,
                false,
                1,
                None,
            )
            .await;
            tracing::info!("Finished - {:?}", result.is_ok());
            result
        }
        .instrument(tracing::info_span!("Process 2")),
    );

    let (result1, result2) = tokio::join!(handle1, handle2);

    // Both should succeed
    let result1 = result1.unwrap();
    if let Err(e) = &result1 {
        eprintln!("Process 1 error: {:?}", e);
    }
    assert!(result1.is_ok(), "Process 1 should succeed");

    let result2 = result2.unwrap();
    if let Err(e) = &result2 {
        eprintln!("Process 2 error: {:?}", e);
    }
    assert!(result2.is_ok(), "Process 2 should succeed after retry");

    // Verify repodata is valid
    let repodata: serde_json::Value =
        serde_json::from_slice(&op.read("noarch/repodata.json").await.unwrap().to_vec()).unwrap();

    assert_eq!(repodata["info"]["subdir"], "noarch");
    // Check that we have exactly one package (either in packages or packages.conda)
    let packages_count = repodata["packages"]
        .as_object()
        .map(|o| o.len())
        .unwrap_or(0);
    let conda_packages_count = repodata["packages.conda"]
        .as_object()
        .map(|o| o.len())
        .unwrap_or(0);
    assert_eq!(
        packages_count + conda_packages_count,
        1,
        "Expected 1 package in repodata"
    );
    println!("Final repodata created successfully");
}
