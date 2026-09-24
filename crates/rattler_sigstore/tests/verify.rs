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

/// Lays out a local channel and returns its `file://` URL.
fn file_channel(dir: &Path, sidecar_bytes: &[u8]) -> Url {
    let channel = dir.join("channel");
    let subdir = channel.join("osx-arm64");
    std::fs::create_dir_all(&subdir).unwrap();
    std::fs::write(
        subdir.join(format!("{PACKAGE}.sigs.{}", hex::encode(sidecar_hash()))),
        sidecar_bytes,
    )
    .unwrap();
    Url::from_directory_path(channel).unwrap()
}

fn publisher() -> Publisher {
    Publisher::new()
        .with_identity("https://github.com/hunger/octoconda/*")
        .with_issuer(Issuer::github_actions())
}

#[test]
fn sidecar_preserves_encoded_filename() {
    let package =
        Url::parse("https://example.com/channel/noarch/foo-1%21post1-0.conda?token=abc").unwrap();
    let actual = rattler_sigstore::sidecar_url(&package, &sidecar_hash()).unwrap();
    assert_eq!(
        actual.path(),
        format!("{}.sigs.{}", package.path(), hex::encode(sidecar_hash()))
    );
    assert_eq!(actual.query(), package.query());
}

#[tokio::test]
async fn publisher_policy_applies_across_download_hosts() {
    let dir = tempfile::tempdir().unwrap();
    let download_channel = serve_channel(dir.path(), SIDECAR).await;
    let mut record = record(&download_channel, Some(sidecar_hash()));
    let source_channel = Url::parse("https://prefix.dev/github-releases/").unwrap();
    record.channel = Some(source_channel.to_string());
    let config = VerificationConfig::new(
        Publisher::new().with_identity("https://github.com/required-owner/*"),
    );
    let result = verify_record_with_trusted_root(
        &VerificationPolicy::Require(config),
        &record,
        &client(),
        &trusted_root(),
    )
    .await;
    assert!(
        matches!(result, Err(SigstoreError::VerificationFailed { .. })),
        "{result:?}"
    );

    let config = VerificationConfig::new(publisher());
    let result = verify_record_with_trusted_root(
        &VerificationPolicy::Require(config),
        &record,
        &client(),
        &trusted_root(),
    )
    .await
    .unwrap();
    assert!(result.is_verified());
}

#[tokio::test]
async fn sidecar_warnings_redact_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let channel = serve(dir.path()).await;
    let channel = channel.join("/t/TEST_SECRET/channel/").unwrap();
    let record = record(&channel, Some(sidecar_hash()));
    let config = VerificationConfig::new(publisher());
    let result = verify_record_with_trusted_root(
        &VerificationPolicy::Warn(config),
        &record,
        &client(),
        &trusted_root(),
    )
    .await
    .unwrap();
    assert_eq!(result.warnings.len(), 1);
    assert!(result.warnings[0].contains("404"));
    assert!(!result.warnings[0].contains("TEST_SECRET"));
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
async fn fetch_sidecar_supports_bounded_file_urls() {
    let dir = tempfile::tempdir().unwrap();
    let channel = file_channel(dir.path(), SIDECAR);
    let record = record(&channel, Some(sidecar_hash()));

    let sidecar = fetch_sidecar(&client(), &record, 1 << 20)
        .await
        .unwrap()
        .expect("record advertises attestations");
    assert_eq!(sidecar.sha256, sidecar_hash());
    assert_eq!(sidecar.bundles.len(), 1);

    let err = fetch_sidecar(&client(), &record, 1024).await.unwrap_err();
    assert!(
        matches!(err, SigstoreError::SidecarTooLarge { .. }),
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

    // The signing certificate locates the package in the source it was built
    // from, and the log entry locates the signature in a public log.
    let claims = attestation.claims().expect("a CI signing certificate");
    assert_eq!(
        claims.source_repository_uri.as_deref(),
        Some("https://github.com/hunger/octoconda")
    );
    assert_eq!(
        claims.run_invocation_uri.as_deref(),
        Some("https://github.com/hunger/octoconda/actions/runs/23778256205/attempts/1")
    );
    assert_eq!(claims.runner_environment.as_deref(), Some("github-hosted"));

    // Fulcio issues certificates with a ten minute lifetime, which is why
    // `not_before` is a usable approximation of the signing time.
    let certificate = attestation
        .certificate
        .as_ref()
        .expect("a Fulcio signing certificate");
    assert_eq!(
        certificate.not_after.duration_since(certificate.not_before),
        jiff::SignedDuration::from_mins(10)
    );

    assert_eq!(attestation.log_index(), Some(1_202_156_555));
    assert_eq!(
        attestation.log_origin(),
        Some("rekor.sigstore.dev - 1193050959916656506")
    );

    let checks = attestation.checks;
    assert!(checks.certificate_chain);
    assert!(checks.signed_certificate_timestamp);
    assert!(checks.transparency_log);
    assert!(checks.inclusion_proof);
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
    let config = || VerificationConfig::new(publisher()).with_channel_check(ChannelCheck::Ignore);

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
    let wrong_publisher = VerificationConfig::new(
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

    // An explicitly unconstrained publisher accepts a valid signature.
    let any_publisher =
        VerificationConfig::new(Publisher::new()).with_channel_check(ChannelCheck::Ignore);
    let outcome = verify_record_with_trusted_root(
        &VerificationPolicy::Require(any_publisher),
        &record,
        &client(),
        &root,
    )
    .await
    .unwrap();
    assert!(outcome.is_verified());

    // Disabled never touches anything.
    let outcome =
        verify_record_with_trusted_root(&VerificationPolicy::Disabled, &record, &client(), &root)
            .await
            .unwrap();
    assert!(!outcome.is_verified());
}
