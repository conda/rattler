//! Live write-path integration tests against a local Azurite emulator.
//!
//! `index_azure` builds its opendal config from a channel URL plus one
//! `azure-options` entry, and under `path-style = true` the account moves from the
//! host into the endpoint path. Unit tests can only assert the strings that
//! construction produces; whether a real Azure Blob implementation accepts them is
//! a different question, and this is where it gets answered:
//!
//! ```toml
//! [azure-options."127.0.0.1:10000"]
//! auth = true
//! scheme = "http"
//! path-style = true
//! ```
//!
//! The rest of the file covers two opendal behaviours that the production code
//! deliberately works around, and that only a real server can demonstrate: the
//! multi-block write path silently ignores `if_not_exists`, and it does carry
//! `Cache-Control` through its commit.
//!
//! Run with:
//!
//! ```text
//! docker run --rm -p 10000:10000 mcr.microsoft.com/azure-storage/azurite \
//!     azurite-blob --blobHost 0.0.0.0
//! cargo nextest run -p rattler_index --test azure_azurite --run-ignored all
//! ```
//!
//! No `--skipApiVersionCheck` needed: the `x-ms-version` opendal pins is older
//! than what current Azurite accepts. Verified on 3.36.0, which answers that
//! version with `AuthorizationFailure` rather than `InvalidHeaderValue`, i.e. it
//! validates the signature instead of rejecting the version. Add the flag only if
//! an older emulator rejects the version outright.
#![cfg(feature = "azure")]

use std::{collections::HashMap, path::PathBuf};

use opendal::{Configurator, ErrorKind, Operator, services::AzblobConfig};
use rattler_azure::{
    Addressing, Auth, AzureChannelUrl, AzureCredentials, AzureEndpointOptions, AzureHost,
    AzureScheme,
};
use rattler_index::{IndexAzureConfig, PackageRevisionAssignment, index_azure};

/// Azurite's development account and its fixed key. Not a secret: both are
/// published constants of the emulator, hardcoded in opendal's own source, and
/// they only ever address a loopback port.
const ACCOUNT: &str = "devstoreaccount1";
const ACCOUNT_KEY: &str =
    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

/// The authority, which is also the exact `azure-options` table key. An IP with a
/// port is precisely the host shape host-style addressing cannot read an account
/// out of, so it only works through a `path-style = true` entry.
const AUTHORITY: &str = "127.0.0.1:10000";

const CONTAINER: &str = "test-channel";

/// The `Cache-Control` `rattler_index` writes on repodata (`lib.rs`'s
/// `CACHE_CONTROL_REPODATA`), duplicated because the constant is private.
const CACHE_CONTROL_REPODATA: &str = "public, max-age=300";

const MIB: usize = 1024 * 1024;

/// `rattler_upload`'s `DESIRED_CHUNK_SIZE`, which is what decides whether a
/// package upload takes opendal's single-shot or multi-block path.
const UPLOAD_CHUNK_SIZE: usize = 10 * MIB;

const PACKAGE: &str = "empty-0.1.0-h4616a5c_0.conda";

fn package_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data/packages")
        .join(PACKAGE)
}

/// The channel as a user would write it: the account is the first path segment,
/// which is what `path-style = true` means. `prefix` keeps each test in its own
/// subtree so they can run in parallel.
fn channel(prefix: &str) -> AzureChannelUrl {
    AzureChannelUrl::parse(&format!("az://{AUTHORITY}/{ACCOUNT}/{CONTAINER}/{prefix}"))
        .expect("azurite channel url")
}

/// The `azure-options` entry for the emulator: the only configuration these tests
/// hand to the indexer.
fn azurite_options() -> AzureEndpointOptions {
    AzureEndpointOptions {
        auth: Auth::DefaultChain,
        scheme: AzureScheme::Http,
        addressing: Addressing::PathStyle,
    }
}

/// An operator built exactly the way `index_azure` builds one, so the opendal-level
/// tests below run against the config the production path derives rather than a
/// hand-written stand-in.
fn production_operator(channel: &AzureChannelUrl) -> Operator {
    let config = rattler_azure::azblob_config(
        &AzureCredentials::AccountKey(ACCOUNT_KEY.to_string()),
        channel,
        azurite_options(),
    )
    .expect("azblob config for an azurite path-style channel");
    Operator::new(config.into_builder())
        .expect("azblob operator")
        .finish()
}

/// An operator written out by hand, *not* derived from the code under test.
///
/// This is what makes the round-trip assertion mean something: if
/// `azblob_config`'s path-style derivation put the blobs somewhere else, this
/// operator would not find them. It is the verbatim shape the emulator wants — the
/// account appears both inside `endpoint` and in `account_name`, `container` is
/// separate, and `root` is the channel prefix without repeating the container.
fn verify_operator(prefix: &str) -> Operator {
    let config = AzblobConfig {
        endpoint: Some(format!("http://{AUTHORITY}/{ACCOUNT}")),
        account_name: Some(ACCOUNT.to_string()),
        account_key: Some(ACCOUNT_KEY.to_string()),
        container: CONTAINER.to_string(),
        root: Some(format!("/{prefix}")),
        ..Default::default()
    };
    Operator::new(config.into_builder())
        .expect("azblob operator")
        .finish()
}

/// Create the channel's container, which Azurite never does implicitly.
///
/// This goes through `AzureMiddleware` because it is the one signer already
/// reachable from here: opendal exposes no container-creation operation, and
/// hand-rolling shared-key signing in a test fixture would be more code than the
/// tests it supports.
async fn ensure_container() {
    let options = HashMap::from([(
        AzureHost::parse(AUTHORITY).expect("azurite authority is a valid host:port"),
        azurite_options(),
    )]);
    let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
        .with(rattler_networking::AzureMiddleware::new(
            reqwest::Client::new(),
            options,
        ))
        .build();

    let created = client
        .put(format!(
            "az://{AUTHORITY}/{ACCOUNT}/{CONTAINER}?restype=container"
        ))
        .send()
        .await
        .expect("container create request failed");
    assert!(
        // 409 is `ContainerAlreadyExists` — a re-run, not a failure.
        created.status().is_success() || created.status() == reqwest::StatusCode::CONFLICT,
        "could not create container {CONTAINER}: {}",
        created.status()
    );
}

/// Run `body` with the emulator credentials in the environment.
///
/// reqsign's env provider sits first in its default chain, so a shared key is how
/// an `auth = true` grant resolves against Azurite — it rejects the AAD bearer
/// tokens the rest of the chain produces. The chain itself is left alone.
async fn with_azurite_credentials<F: Future<Output = ()>>(body: F) {
    temp_env::async_with_vars(
        [
            ("AZURE_STORAGE_ACCOUNT_NAME", Some(ACCOUNT)),
            ("AZURE_STORAGE_ACCOUNT_KEY", Some(ACCOUNT_KEY)),
        ],
        body,
    )
    .await;
}

fn index_config(channel: AzureChannelUrl) -> IndexAzureConfig {
    IndexAzureConfig {
        channel,
        credentials: AzureCredentials::AccountKey(ACCOUNT_KEY.to_string()),
        options: azurite_options(),
        target_platform: None,
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        force: true,
        max_parallel: 4,
        multi_progress: None,
    }
}

/// The round trip: seed a package into a path-style Azurite channel, index it
/// through an `azure-options` entry, and read the written `repodata.json` back.
///
/// The read side uses the hand-written operator, so this checks that the indexer
/// wrote to the blob the URL names — not merely that it reported success.
#[tokio::test]
#[ignore = "requires a running Azurite emulator; see the module docs"]
async fn azurite_index_round_trip_through_a_path_style_entry() {
    with_azurite_credentials(async {
        const PREFIX: &str = "round-trip";
        ensure_container().await;

        let seeded = verify_operator(PREFIX);
        seeded
            .write(
                &format!("noarch/{PACKAGE}"),
                fs_err::read(package_path()).expect("test package"),
            )
            .await
            .expect("seeding the package failed");

        index_azure(index_config(channel(PREFIX)))
            .await
            .expect("indexing an azurite channel failed");

        let repodata = seeded
            .read("noarch/repodata.json")
            .await
            .expect("the indexer wrote no repodata.json where the channel URL points");
        let json: serde_json::Value =
            serde_json::from_slice(&repodata.to_vec()).expect("repodata was not valid json");
        assert!(
            json["packages.conda"]
                .as_object()
                .is_some_and(|packages| packages.contains_key(PACKAGE)),
            "repodata should list the seeded package: {json}"
        );
        assert_eq!(
            json["info"]["subdir"], "noarch",
            "repodata should describe the subdir it was written for: {json}"
        );

        // The same write path that ros-recipes' cache-header sweep exists to
        // override. Live evidence that opendal's azblob backend honours it at all.
        let metadata = seeded
            .stat("noarch/repodata.json")
            .await
            .expect("stat repodata.json");
        assert_eq!(metadata.cache_control(), Some(CACHE_CONTROL_REPODATA));
    })
    .await;
}

/// A write larger than the chunk size commits through Put Block List, and
/// `Cache-Control` survives that commit.
///
/// Nothing in production reaches this yet. opendal's azblob backend declares no
/// `write_multi_min_size`, so `Operator::write_with` hands the whole buffer over in
/// a single `write` call and always takes the single-shot Put Blob path, however
/// large the repodata gets; only an explicit `chunk` splits it. So this locks the
/// header down for the commit path a chunked repodata write would take, which is
/// the gap that made the live `Cache-Control` evidence incomplete.
#[tokio::test]
#[ignore = "requires a running Azurite emulator; see the module docs"]
async fn azurite_multi_block_write_keeps_cache_control() {
    with_azurite_credentials(async {
        const PREFIX: &str = "multi-block-cache-control";
        ensure_container().await;
        let op = production_operator(&channel(PREFIX));

        // Two chunks' worth, so `write` is called more than once and the writer
        // switches from `write_once` to staging blocks.
        let mut writer = op
            .writer_with("noarch/repodata.json")
            .chunk(2 * MIB)
            .cache_control(CACHE_CONTROL_REPODATA)
            .await
            .expect("opening a chunked writer failed");
        writer
            .write(vec![b'{'; 5 * MIB])
            .await
            .expect("chunked write failed");
        writer.close().await.expect("Put Block List commit failed");

        let metadata = op
            .stat("noarch/repodata.json")
            .await
            .expect("stat the multi-block blob");
        assert!(
            metadata.content_length() == (5 * MIB) as u64,
            "the blob should have committed all blocks, got {} bytes",
            metadata.content_length()
        );
        assert_eq!(
            metadata.cache_control(),
            Some(CACHE_CONTROL_REPODATA),
            "Put Block List should carry x-ms-blob-cache-control through its commit"
        );
    })
    .await;
}

/// The gap `upload_package_to_azure`'s pre-write `stat` exists to close: opendal
/// honours `if_not_exists` on the single-shot Put Blob path and silently drops it
/// on the multi-block path.
///
/// A package over `rattler_upload`'s 10 MiB chunk size is the only way to reach the
/// multi-block path, and this is as close as a test can currently get to that
/// upload: `rattler_upload` reads no config file, so it always builds a host-style
/// endpoint and cannot be pointed at an emulator at all. What it can share is the
/// operator, built here through the same `azblob_config`, and the uploader's chunk
/// size and overwrite guard — so the behaviour being reproduced is the uploader's,
/// even though the entry point is not.
#[tokio::test]
#[ignore = "requires a running Azurite emulator; see the module docs"]
async fn azurite_if_not_exists_is_dropped_on_the_multi_block_path() {
    with_azurite_credentials(async {
        const PREFIX: &str = "overwrite-guard";
        ensure_container().await;
        let op = production_operator(&channel(PREFIX));

        // Baseline: below the chunk size, the guard works and opendal reports the
        // conflict the uploader turns into "already exists, use --force".
        let small = "noarch/small.conda";
        op.write(small, b"first".to_vec())
            .await
            .expect("small write failed");
        let refused = op
            .write_with(small, b"second".to_vec())
            .if_not_exists(true)
            .await
            .expect_err("if_not_exists should refuse to overwrite a small blob");
        assert_eq!(refused.kind(), ErrorKind::ConditionNotMatch);

        // Over the chunk size, the same option is accepted and then ignored.
        let large = "noarch/large.conda";
        let payload = vec![0u8; UPLOAD_CHUNK_SIZE + 2 * MIB];
        write_chunked(&op, large, &payload, false).await;
        write_chunked(&op, large, &payload, true).await;

        // Which is why the uploader stats first. That check does see the blob, so
        // the guard holds for large packages despite opendal dropping the option.
        assert!(
            op.stat(large).await.is_ok(),
            "the pre-write stat must see an existing large blob, since if_not_exists does not"
        );
    })
    .await;
}

/// Write `payload` the way `upload_single_package` does, and assert it succeeded.
///
/// `guard` is that function's `if_not_exists(!force)`. Passing `true` over a blob
/// that already exists still succeeds, which is the point being demonstrated: on
/// the multi-block path the option is a no-op, so a large upload would clobber
/// without the separate `stat`.
async fn write_chunked(op: &Operator, path: &str, payload: &[u8], guard: bool) {
    let mut writer = op
        .writer_with(path)
        .chunk(UPLOAD_CHUNK_SIZE)
        .if_not_exists(guard)
        .await
        .expect("opening a chunked writer failed");
    writer
        .write(payload.to_vec())
        .await
        .expect("chunked write failed");
    writer
        .close()
        .await
        .expect("a multi-block commit should succeed even with if_not_exists set");
}
