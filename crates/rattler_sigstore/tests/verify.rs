//! End-to-end tests: serve a sidecar over HTTP, discover it through a record,
//! and verify the real bundle it contains against the embedded trusted root.

use std::{future::IntoFuture, net::SocketAddr, path::Path, str::FromStr};

use rattler_conda_types::{
    PackageRecord, RepoDataRecord, VersionWithSource,
    package::{ArchiveIdentifier, DistArchiveIdentifier},
};
use rattler_digest::{Sha256, Sha256Hash};
use rattler_sigstore::{
    ChannelCheck, Issuer, Publisher, SigstoreError, TrustedRoot, VerificationConfig,
    VerificationPolicy, fetch_sidecar, verify_bundles, verify_record_with_trusted_root,
};
use reqwest_middleware::ClientWithMiddleware;
use sigstore_verify::trust_root::SigstoreInstance;
use tower_http::services::ServeDir;
use url::Url;

const PACKAGE: &str = "actionlint-1.7.12-h60d57d3_0.conda";
const PACKAGE_SHA256: &str = "e3e0f35dec5b09b18baac8729d14115903b5adfd25065f8bbb90a2b3be5401e4";
const IDENTITY: &str =
    "https://github.com/hunger/octoconda/.github/workflows/octoconda.yaml@refs/heads/main";
const SIDECAR: &[u8] = include_bytes!("../test-data/actionlint-1.7.12-h60d57d3_0.conda.sigs");

fn sidecar_hash() -> Sha256Hash {
    rattler_digest::compute_bytes_digest::<Sha256>(SIDECAR)
}

fn trusted_root() -> TrustedRoot {
    TrustedRoot::from_embedded(SigstoreInstance::PublicGood).unwrap()
}

fn client() -> ClientWithMiddleware {
    reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build()
}

/// A record for the fixture package as it would come out of the repodata of
/// `channel_url`.
fn record(channel_url: &Url, attestations_sha256: Option<Sha256Hash>) -> RepoDataRecord {
    let identifier = DistArchiveIdentifier::try_from_filename(PACKAGE).unwrap();
    let ArchiveIdentifier {
        name,
        version,
        build_string,
    } = identifier.identifier.clone();
    let mut package_record = PackageRecord::new(
        name.parse().unwrap(),
        VersionWithSource::from_str(&version).unwrap(),
        build_string,
    );
    package_record.subdir = "osx-arm64".to_string();
    package_record.sha256 =
        Some(rattler_digest::parse_digest_from_hex::<Sha256>(PACKAGE_SHA256).unwrap());
    package_record.attestations_sha256 = attestations_sha256;
    RepoDataRecord {
        package_record,
        identifier,
        url: channel_url.join(&format!("osx-arm64/{PACKAGE}")).unwrap(),
        channel: Some(channel_url.to_string()),
    }
}

/// Serves `dir` on a random local port and returns its base URL.
async fn serve(dir: &Path) -> Url {
    let app = axum::Router::new().fallback_service(ServeDir::new(dir));
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, app.into_make_service()).into_future());
    Url::parse(&format!("http://127.0.0.1:{}/channel/", addr.port())).unwrap()
}

/// Lays out a channel directory with the fixture sidecar at its
/// content-addressed location and returns the channel URL.
async fn serve_channel(dir: &Path, sidecar_bytes: &[u8]) -> Url {
    let subdir = dir.join("channel").join("osx-arm64");
    std::fs::create_dir_all(&subdir).unwrap();
    std::fs::write(
        subdir.join(format!("{PACKAGE}.sigs.{}", hex::encode(sidecar_hash()))),
        sidecar_bytes,
    )
    .unwrap();
    serve(dir).await
}

fn publisher() -> Publisher {
    Publisher::new()
        .with_identity("https://github.com/hunger/octoconda/*")
        .with_issuer(Issuer::github_actions())
}

#[tokio::test]
async fn fetch_sidecar_discovers_through_record() {
    let dir = tempfile::tempdir().unwrap();
    let channel = serve_channel(dir.path(), SIDECAR).await;

    let sidecar = fetch_sidecar(&client(), &record(&channel, Some(sidecar_hash())), 1 << 20)
        .await
        .unwrap()
        .expect("record advertises attestations");
    assert_eq!(sidecar.sha256, sidecar_hash());
    assert_eq!(sidecar.bundles.len(), 1);

    // A record without the field never hits the network.
    let none = fetch_sidecar(&client(), &record(&channel, None), 1 << 20)
        .await
        .unwrap();
    assert!(none.is_none());
}

#[tokio::test]
async fn fetch_sidecar_reports_retrieval_failures() {
    let dir = tempfile::tempdir().unwrap();
    let channel = serve_channel(dir.path(), SIDECAR).await;

    // Advertised hash points at a file that is not served.
    let other_hash = rattler_digest::compute_bytes_digest::<Sha256>(b"other");
    let err = fetch_sidecar(&client(), &record(&channel, Some(other_hash)), 1 << 20)
        .await
        .unwrap_err();
    assert!(
        matches!(err, SigstoreError::SidecarHttpStatus { status, .. } if status == 404),
        "{err}"
    );

    // Size limit is enforced.
    let err = fetch_sidecar(&client(), &record(&channel, Some(sidecar_hash())), 1024)
        .await
        .unwrap_err();
    assert!(
        matches!(err, SigstoreError::SidecarTooLarge { .. }),
        "{err}"
    );

    // Served bytes that do not match the advertised hash are rejected.
    let dir = tempfile::tempdir().unwrap();
    let mut tampered = SIDECAR.to_vec();
    tampered.extend_from_slice(b" ");
    let channel = serve_channel(dir.path(), &tampered).await;
    let err = fetch_sidecar(&client(), &record(&channel, Some(sidecar_hash())), 1 << 20)
        .await
        .unwrap_err();
    assert!(
        matches!(err, SigstoreError::SidecarHashMismatch { .. }),
        "{err}"
    );
}

#[tokio::test]
async fn verify_real_bundle_offline() {
    let channel = Url::parse("https://prefix.dev/github-releases/").unwrap();
    let record = record(&channel, Some(sidecar_hash()));
    let sidecar = rattler_sigstore::parse_sidecar(&record.url, SIDECAR, &sidecar_hash()).unwrap();

    let result = verify_bundles(
        &record,
        &sidecar.bundles,
        ChannelCheck::Require,
        &trusted_root(),
    )
    .unwrap();
    assert!(result.rejected.is_empty(), "{:?}", result.rejected);
    let attestation = &result.verified[0];
    assert_eq!(attestation.identity.as_deref(), Some(IDENTITY));
    assert_eq!(
        attestation.issuer.as_deref(),
        Some(Issuer::github_actions().as_str())
    );
    assert_eq!(
        attestation.target_channel.as_deref(),
        Some("https://prefix.dev/github-releases")
    );
    assert!(attestation.integrated_time.is_some());
    assert!(
        attestation.warnings.is_empty(),
        "{:?}",
        attestation.warnings
    );
}

#[tokio::test]
async fn verify_rejects_wrong_package_digest() {
    let channel = Url::parse("https://prefix.dev/github-releases/").unwrap();
    let mut record = record(&channel, Some(sidecar_hash()));
    record.package_record.sha256 = Some(rattler_digest::compute_bytes_digest::<Sha256>(b"evil"));
    let sidecar = rattler_sigstore::parse_sidecar(&record.url, SIDECAR, &sidecar_hash()).unwrap();

    let result = verify_bundles(
        &record,
        &sidecar.bundles,
        ChannelCheck::Require,
        &trusted_root(),
    )
    .unwrap();
    assert!(result.verified.is_empty());
    assert!(
        result.rejected[0]
            .reason
            .contains("does not match any subject"),
        "{}",
        result.rejected[0].reason
    );
}

#[tokio::test]
async fn verify_checks_target_channel() {
    // Same package served from a mirror: the attestation targets prefix.dev.
    let mirror = Url::parse("https://mirror.example.com/github-releases/").unwrap();
    let record = record(&mirror, Some(sidecar_hash()));
    let sidecar = rattler_sigstore::parse_sidecar(&record.url, SIDECAR, &sidecar_hash()).unwrap();

    let strict = verify_bundles(
        &record,
        &sidecar.bundles,
        ChannelCheck::Require,
        &trusted_root(),
    )
    .unwrap();
    assert!(strict.verified.is_empty());
    assert!(strict.rejected[0].reason.contains("targets channel"));

    let warn = verify_bundles(
        &record,
        &sidecar.bundles,
        ChannelCheck::Warn,
        &trusted_root(),
    )
    .unwrap();
    assert_eq!(warn.verified.len(), 1);
    assert_eq!(warn.verified[0].warnings.len(), 1);

    let ignore = verify_bundles(
        &record,
        &sidecar.bundles,
        ChannelCheck::Ignore,
        &trusted_root(),
    )
    .unwrap();
    assert_eq!(ignore.verified.len(), 1);
    assert!(ignore.verified[0].warnings.is_empty());
}

#[tokio::test]
async fn verify_record_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let channel = serve_channel(dir.path(), SIDECAR).await;
    let record = record(&channel, Some(sidecar_hash()));
    let root = trusted_root();

    // The package is served from a local test server, so the attestation's
    // targetChannel does not match: relax the channel check for this test.
    let config = || {
        VerificationConfig::new()
            .with_channel_publisher(channel.clone(), publisher())
            .with_channel_check(ChannelCheck::Ignore)
    };

    let outcome = verify_record_with_trusted_root(
        &VerificationPolicy::Require(config()),
        &record,
        &client(),
        &root,
    )
    .await
    .unwrap();
    assert!(outcome.is_verified());
    assert_eq!(
        outcome.attestation.unwrap().identity.as_deref(),
        Some(IDENTITY)
    );

    // A publisher that does not match the signer rejects the package.
    let wrong_publisher = VerificationConfig::new()
        .with_channel_publisher(
            channel.clone(),
            Publisher::new().with_identity("https://github.com/someone-else/*"),
        )
        .with_channel_check(ChannelCheck::Ignore);
    let err = verify_record_with_trusted_root(
        &VerificationPolicy::Require(wrong_publisher.clone()),
        &record,
        &client(),
        &root,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, SigstoreError::VerificationFailed { .. }),
        "{err}"
    );

    // In warn mode the same failure is a warning.
    let outcome = verify_record_with_trusted_root(
        &VerificationPolicy::Warn(wrong_publisher),
        &record,
        &client(),
        &root,
    )
    .await
    .unwrap();
    assert!(!outcome.is_verified());
    assert_eq!(outcome.warnings.len(), 1);

    // A record that advertises no attestations fails closed in require mode.
    let err = verify_record_with_trusted_root(
        &VerificationPolicy::Require(config()),
        &self::record(&channel, None),
        &client(),
        &root,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, SigstoreError::NoAttestationsAdvertised(_)),
        "{err}"
    );

    // An unmapped channel without default publishers fails closed in require
    // mode and is skipped in warn mode.
    let unmapped = VerificationConfig::new();
    let err = verify_record_with_trusted_root(
        &VerificationPolicy::Require(unmapped.clone()),
        &record,
        &client(),
        &root,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, SigstoreError::NoPublishersConfigured(_)),
        "{err}"
    );
    let outcome = verify_record_with_trusted_root(
        &VerificationPolicy::Warn(unmapped),
        &record,
        &client(),
        &root,
    )
    .await
    .unwrap();
    assert!(!outcome.is_verified());
    assert!(outcome.warnings.is_empty());

    // Disabled never touches anything.
    let outcome =
        verify_record_with_trusted_root(&VerificationPolicy::Disabled, &record, &client(), &root)
            .await
            .unwrap();
    assert!(!outcome.is_verified());
}
