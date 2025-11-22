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
use rattler_index::{index, PreconditionChecks};
use tracing::Instrument;

use super::etag_memory_backend::ETagMemoryBuilder;

/// Validates that concurrent indexing properly handles race conditions with retry logic.
///
/// This test creates a deterministic race condition where two processes attempt to
/// update the same repodata.json simultaneously. It verifies that:
/// - The first process completes successfully
/// - The second process detects the race condition (`ETag` mismatch)
/// - The second process retries and eventually succeeds
/// - Exactly one retry occurs (deterministic due to synchronization barriers)
/// - The final repodata.json is valid and contains the expected package
#[tokio::test]
async fn test_concurrent_index_with_race_condition_and_retry() {
    use std::sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    };
    use tokio::sync::Barrier;

    // Initialize tracing subscriber to see log output
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .compact()
        .with_target(false)
        .try_init();

    // Create a state machine to force deterministic race condition:
    // Block until both processes stat repodata.json (only first 2 stats, not on
    // retry) The backend's per-file locks ensure writes happen sequentially

    let stat_barrier = Arc::new(Barrier::new(2));
    let stat_barrier_clone = stat_barrier.clone();

    // Count how many times the stat has been called - only wait at barrier for
    // first 2
    let stat_count = Arc::new(AtomicU8::new(0));
    let stat_count_clone = stat_count.clone();

    let test_hooks = super::etag_memory_backend::TestHooks {
        on_operation: Arc::new(move |path, op| {
            let stat_barrier = stat_barrier_clone.clone();
            let stat_count = stat_count_clone.clone();
            let path = path.to_string();
            Box::pin(async move {
                if path == "noarch/repodata.json"
                    && op == super::etag_memory_backend::Operation::AfterStat
                {
                    let count = stat_count.fetch_add(1, Ordering::SeqCst);
                    if count < 2 {
                        // First two stats - wait at barrier
                        tracing::info!("Process {} reached stat barrier", count + 1);
                        stat_barrier.wait().await;
                        tracing::info!("Both processes statted, continuing");
                    } else {
                        tracing::info!("Process on retry (stat {}), skipping barrier", count + 1);
                    }
                }
            })
        }),
    };

    // Create operator with ETag support and test hooks
    let op = Operator::new(ETagMemoryBuilder::default().with_test_hooks(test_hooks))
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

    // Process 1: will write first
    let handle1 = tokio::spawn(
        async move {
            let result = index(
                Some(Platform::NoArch),
                op1,
                None,
                false,
                false,
                false,
                1,
                None,
                PreconditionChecks::Enabled,
            )
            .await;
            result
        }
        .instrument(tracing::info_span!("Process 1")),
    );

    // Process 2: will encounter race condition and retry
    let handle2 = tokio::spawn(
        async move {
            let result = index(
                Some(Platform::NoArch),
                op2,
                None,
                false,
                false,
                false,
                1,
                None,
                PreconditionChecks::Enabled,
            )
            .await;
            result
        }
        .instrument(tracing::info_span!("Process 2")),
    );

    let (result1, result2) = tokio::join!(handle1, handle2);

    // Both should succeed
    let stats1 = result1.unwrap().unwrap();
    let stats2 = result2.unwrap().unwrap();

    // Verify that exactly one process had to retry
    let total_retries = stats1.subdirs.values().map(|s| s.retries).sum::<usize>()
        + stats2.subdirs.values().map(|s| s.retries).sum::<usize>();
    assert_eq!(
        total_retries, 1,
        "Expected exactly 1 retry due to deterministic race condition, but got {total_retries}"
    );

    // Verify repodata is valid
    let repodata: serde_json::Value =
        serde_json::from_slice(&op.read("noarch/repodata.json").await.unwrap().to_vec()).unwrap();

    assert_eq!(repodata["info"]["subdir"], "noarch");
    // Check that we have exactly one package (either in packages or packages.conda)
    let packages_count = repodata["packages"]
        .as_object()
        .map_or(0, serde_json::Map::len);
    let conda_packages_count = repodata["packages.conda"]
        .as_object()
        .map_or(0, serde_json::Map::len);
    assert_eq!(
        packages_count + conda_packages_count,
        1,
        "Expected 1 package in repodata"
    );

    tracing::info!("Test completed successfully with exactly 1 retry");
}
