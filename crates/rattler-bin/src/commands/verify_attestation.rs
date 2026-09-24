//! Verify a Sigstore attestation sidecar against a conda package archive.

use std::{ffi::OsString, path::Path};

use console::style;
use futures_util::StreamExt;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{RepoDataRecord, package::DistArchiveIdentifier};
use rattler_package_streaming::fs::repodata_record_from_package_archive;
use rattler_redaction::Redact;
use rattler_sigstore::{
    CONDA_PUBLISH_PREDICATE_TYPE, CertificateClaims, ChannelCheck, DEFAULT_MAX_SIDECAR_SIZE,
    RejectedAttestation, SigstoreError, VerifiedAttestation, VerifiedChecks, embedded_trusted_root,
    fetch_bundles, mutable_sidecar_url, production_trusted_root, verify_bundles,
};
use reqwest_middleware::ClientWithMiddleware;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use url::Url;

use super::{client::create_client_with_middleware, package_source::PackageSource};
use crate::publisher_args::PublisherArgs;

/// Verify Sigstore attestations for a conda package.
#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// Path or URL of the conda package archive (.conda or .tar.bz2).
    #[clap(value_name = "PACKAGE")]
    package: String,

    /// Path or URL of the attestation sidecar.
    ///
    /// Defaults to `<PACKAGE>.sigs`, with the suffix appended before a URL's
    /// query string.
    #[clap(long, value_name = "PATH_OR_URL")]
    attestation: Option<String>,

    /// Expected source channel for the attestation's `targetChannel`.
    ///
    /// For remote packages this defaults to the channel inferred from the
    /// package URL. For local packages, `targetChannel` is not compared unless
    /// this option is supplied.
    #[clap(long, value_name = "URL")]
    channel: Option<Url>,

    /// Signing certificate publisher constraints.
    #[clap(flatten)]
    publisher: PublisherArgs,

    /// Output format (defaults to human-readable output).
    ///
    /// The JSON output contains every claim of the signing certificate and the
    /// full transparency log metadata, not only the summary that is printed for
    /// humans.
    #[clap(long)]
    format: Option<OutputFormat>,
}

/// Machine-readable output formats for attestation verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    /// Output the verification result as JSON.
    Json,
}

/// Verify the attestation sidecar selected by [`Opt`].
pub async fn verify_attestation(opt: Opt, offline: bool) -> miette::Result<()> {
    let package_source = PackageSource::parse(&opt.package);
    let compare_channel = opt.channel.is_some() || package_source.is_url();
    let client = create_client_with_middleware(offline)?;

    let mut record = load_package_record(&package_source, &client).await?;
    if let Some(channel) = opt.channel {
        record.channel = Some(channel.to_string());
    }

    let attestation_source = opt.attestation.as_deref().map_or_else(
        || default_attestation_source(&package_source),
        PackageSource::parse,
    );
    let attestation_url = source_url(&attestation_source)?;
    let bundles = fetch_bundles(&client, &attestation_url, DEFAULT_MAX_SIDECAR_SIZE)
        .await
        .into_diagnostic()?;
    // Fetching the trusted root goes through TUF, which needs the network, so
    // an offline verification falls back to the snapshot embedded in the binary.
    let offline_root = offline
        .then(embedded_trusted_root)
        .transpose()
        .into_diagnostic()?;
    let trusted_root = match &offline_root {
        Some(root) => root,
        None => production_trusted_root().await.into_diagnostic()?,
    };
    let mut verification = verify_bundles(
        &record,
        &bundles,
        if compare_channel {
            ChannelCheck::Require
        } else {
            ChannelCheck::Ignore
        },
        trusted_root,
    )
    .into_diagnostic()?;

    let publisher = opt.publisher.publisher();
    verification.verified.retain(|attestation| {
        let matches = publisher.matches(
            attestation.identity.as_deref(),
            attestation.issuer.as_deref(),
        );
        if !matches {
            verification.rejected.push(RejectedAttestation {
                index: attestation.index,
                reason: format!(
                    "valid signature by {} (issuer {}) does not match the required publisher",
                    attestation
                        .identity
                        .as_deref()
                        .unwrap_or("<unknown identity>"),
                    attestation.issuer.as_deref().unwrap_or("<unknown issuer>"),
                ),
            });
        }
        matches
    });

    let package_name = record.identifier.to_file_name();
    if verification.verified.is_empty() {
        return Err(SigstoreError::VerificationFailed {
            package: package_name,
            reasons: verification
                .rejected
                .into_iter()
                .map(|rejected| format!("bundle {}: {}", rejected.index, rejected.reason))
                .collect(),
        })
        .into_diagnostic();
    }

    let sha256 = record
        .package_record
        .sha256
        .as_ref()
        .expect("successful verification requires a SHA-256 digest");
    let report = Report {
        verified: true,
        package: package_name,
        sha256: hex::encode(sha256),
        attestation_url: attestation_url.redact().to_string(),
        bundles: verification
            .verified
            .iter()
            .map(BundleReport::new)
            .collect(),
        rejected: verification
            .rejected
            .iter()
            .map(|rejected| RejectedReport {
                index: rejected.index,
                reason: rejected.reason.clone(),
            })
            .collect(),
    };

    match opt.format {
        Some(OutputFormat::Json) => println!(
            "{}",
            serde_json::to_string_pretty(&report).into_diagnostic()?
        ),
        None => print_report(&report),
    }

    for bundle in &report.bundles {
        for warning in &bundle.warnings {
            eprintln!("{} {warning}", style("warning:").yellow().bold());
        }
    }
    for rejected in &report.rejected {
        eprintln!(
            "{} bundle {}: {}",
            style("warning:").yellow().bold(),
            rejected.index,
            rejected.reason
        );
    }

    Ok(())
}

/// The result of verifying the attestations of a single package.
#[derive(Debug, Serialize)]
struct Report {
    /// Always true: a report is only produced when an attestation was accepted.
    verified: bool,
    /// The filename the attestations are bound to.
    package: String,
    /// The hex encoded SHA-256 the attestations are bound to.
    sha256: String,
    /// The sidecar the bundles were read from, with credentials removed.
    attestation_url: String,
    /// The bundles that passed verification and the publisher constraints.
    bundles: Vec<BundleReport>,
    /// The bundles that did not, which do not prevent the command from
    /// succeeding as long as one bundle was accepted.
    rejected: Vec<RejectedReport>,
}

/// A single verified attestation.
#[derive(Debug, Serialize)]
struct BundleReport {
    index: usize,
    identity: Option<String>,
    issuer: Option<String>,
    predicate_type: &'static str,
    target_channel: Option<String>,
    certificate: Option<CertificateReport>,
    transparency_log: Option<TransparencyLogReport>,
    checks: VerifiedChecks,
    warnings: Vec<String>,
}

/// The signing certificate of a verified attestation.
#[derive(Debug, Serialize)]
struct CertificateReport {
    /// When the certificate was issued, which approximates the signing time.
    not_before: String,
    /// When the short-lived certificate expired.
    not_after: Option<String>,
    claims: CertificateClaims,
}

/// The transparency log entry recording a verified attestation.
#[derive(Debug, Serialize)]
struct TransparencyLogReport {
    log_index: u64,
    /// The hex encoded identifier of the log's public key.
    log_id: String,
    /// The type of the log entry, e.g. `dsse`.
    kind: &'static str,
    kind_version: &'static str,
    /// The authenticated time the entry was integrated into the log, set only
    /// when log inclusion was verified.
    integrated_time: Option<String>,
    inclusion_proof: Option<InclusionProofReport>,
    /// Where the entry can be inspected, when the log has a known UI.
    url: Option<String>,
}

/// The inclusion proof that ties an entry to a signed log checkpoint.
#[derive(Debug, Serialize)]
struct InclusionProofReport {
    /// The name the log gives itself in its signed checkpoint.
    origin: Option<String>,
    /// The size of the log the proof was computed against.
    tree_size: u64,
    /// The hex encoded Merkle root the proof leads to.
    root_hash: String,
}

/// A bundle that was not accepted.
#[derive(Debug, Serialize)]
struct RejectedReport {
    index: usize,
    reason: String,
}

impl BundleReport {
    fn new(attestation: &VerifiedAttestation) -> Self {
        Self {
            index: attestation.index,
            identity: attestation.identity.clone(),
            issuer: attestation.issuer.clone(),
            predicate_type: CONDA_PUBLISH_PREDICATE_TYPE,
            target_channel: attestation.target_channel.clone(),
            certificate: attestation
                .certificate
                .as_ref()
                .map(|certificate| CertificateReport {
                    not_before: certificate.validity.start.to_string(),
                    not_after: certificate.validity.end.map(|end| end.to_string()),
                    claims: certificate.claims.clone(),
                }),
            transparency_log: TransparencyLogReport::new(attestation),
            checks: attestation.checks,
            warnings: attestation.warnings.clone(),
        }
    }
}

impl TransparencyLogReport {
    fn new(attestation: &VerifiedAttestation) -> Option<Self> {
        let entry = attestation.log_entry.as_ref()?;
        let log_index = entry.log_index.value();
        let origin = attestation.log_origin();
        Some(Self {
            log_index,
            log_id: hex::encode(entry.log_id.key_id.as_bytes()),
            kind: entry.kind_version.kind(),
            kind_version: entry.kind_version.version(),
            integrated_time: attestation.integrated_time.map(|time| time.to_string()),
            inclusion_proof: entry
                .inclusion_proof
                .as_ref()
                .map(|proof| InclusionProofReport {
                    origin: origin.map(str::to_owned),
                    tree_size: proof.tree_size,
                    root_hash: hex::encode(proof.root_hash.as_bytes()),
                }),
            url: transparency_log_url(origin, log_index),
        })
    }
}

/// A page where a transparency log entry can be inspected.
///
/// Only the Sigstore public good instance has a known UI, so entries in another
/// log get no link rather than a guessed one.
fn transparency_log_url(origin: Option<&str>, log_index: u64) -> Option<String> {
    (log_host(origin?) == "rekor.sigstore.dev")
        .then(|| format!("https://search.sigstore.dev/?logIndex={log_index}"))
}

/// The host of a transparency log, taken from the origin it gives itself in its
/// signed checkpoint.
///
/// Rekor states its tree id after the host, as in
/// `rekor.sigstore.dev - 1193050959916656506`, which identifies the log but is
/// not needed to find an entry in it.
fn log_host(origin: &str) -> &str {
    origin.split_whitespace().next().unwrap_or(origin)
}

/// Prints the report for humans: the identity that signed, the source it was
/// built from and where both can be inspected.
fn print_report(report: &Report) {
    println!(
        "{} {}",
        style("✓").green().bold(),
        style("Sigstore attestation verified").green().bold()
    );
    println!();
    field("Package", &report.package);
    field("SHA-256", &report.sha256);
    field("Attestation", &report.attestation_url);

    let total = report.bundles.len();
    for bundle in &report.bundles {
        println!();
        println!(
            "{}",
            style(format!("Bundle {} of {total}", bundle.index + 1)).bold()
        );
        print_bundle(bundle);
    }
}

fn print_bundle(bundle: &BundleReport) {
    indented("Identity", bundle.identity.as_deref().unwrap_or(UNKNOWN));
    indented("Issuer", bundle.issuer.as_deref().unwrap_or(UNKNOWN));

    if let Some(certificate) = &bundle.certificate {
        let claims = &certificate.claims;
        if let Some(repository) = &claims.source_repository_uri {
            // The identifiers do not change when a repository is renamed, so
            // they are what a trust policy should be pinned to.
            let mut annotations = Vec::new();
            if let Some(visibility) = &claims.source_repository_visibility_at_signing {
                annotations.push(visibility.clone());
            }
            if let Some(id) = &claims.source_repository_identifier {
                annotations.push(format!("id {id}"));
            }
            if annotations.is_empty() {
                indented("Repository", repository);
            } else {
                indented(
                    "Repository",
                    &format!("{repository} ({})", annotations.join(", ")),
                );
            }
        }
        if let Some(commit) = &claims.source_repository_digest {
            let reference = claims.source_repository_ref.as_deref();
            indented(
                "Commit",
                &reference.map_or_else(
                    || commit.clone(),
                    |reference| format!("{commit} on {reference}"),
                ),
            );
        }
        if let Some(workflow) = &claims.build_config_uri {
            let workflow = shorten_build_config(workflow, claims);
            indented(
                "Workflow",
                &claims.build_trigger.as_ref().map_or_else(
                    || workflow.clone(),
                    |trigger| format!("{workflow} (trigger: {trigger})"),
                ),
            );
        }
        if let Some(runner) = &claims.runner_environment {
            indented("Runner", runner);
        }
        if let Some(run) = &claims.run_invocation_uri {
            indented("Build", run);
        }
        indented("Signed at", &certificate.not_before);
    }

    if let Some(log) = &bundle.transparency_log {
        let origin = log
            .inclusion_proof
            .as_ref()
            .and_then(|proof| proof.origin.as_deref());
        indented(
            "Transparency log",
            &origin.map_or_else(
                || format!("index {}", log.log_index),
                |origin| format!("index {} on {}", log.log_index, log_host(origin)),
            ),
        );
        if let Some(url) = &log.url {
            continuation(url);
        }
    }

    indented(
        "Target channel",
        bundle.target_channel.as_deref().unwrap_or("<none>"),
    );
    indented("Checks", &describe_checks(&bundle.checks));
}

/// Names the parts of the verification that were performed, so the output does
/// not imply more than was actually checked.
fn describe_checks(checks: &VerifiedChecks) -> String {
    let mut performed = Vec::new();
    if checks.certificate_chain {
        performed.push("certificate chain");
    }
    if checks.signed_certificate_timestamp {
        performed.push("SCT");
    }
    if checks.transparency_log {
        performed.push(if checks.inclusion_proof {
            "log inclusion proof"
        } else {
            "log inclusion promise"
        });
    }
    if performed.is_empty() {
        return "signature only".to_string();
    }
    performed.join(", ")
}

/// Shortens a build config URI to the path within its repository, since the
/// repository and ref are already shown on their own lines.
fn shorten_build_config(build_config_uri: &str, claims: &CertificateClaims) -> String {
    let mut workflow = build_config_uri;
    if let Some(repository) = &claims.source_repository_uri
        && let Some(relative) = workflow
            .strip_prefix(repository.as_str())
            .and_then(|relative| relative.strip_prefix('/'))
    {
        workflow = relative;
    }
    if let Some(reference) = &claims.source_repository_ref
        && let Some(without_ref) = workflow.strip_suffix(&format!("@{reference}"))
    {
        workflow = without_ref;
    }
    workflow.to_string()
}

/// Shown for a value the attestation does not carry.
const UNKNOWN: &str = "<unknown>";

/// The width the labels of the top level fields are padded to.
const LABEL_WIDTH: usize = 12;

/// The width the labels within a bundle are padded to, including indentation.
const BUNDLE_LABEL_WIDTH: usize = 18;

fn field(label: &str, value: &str) {
    println!("{:<LABEL_WIDTH$} {value}", format!("{label}:"));
}

fn indented(label: &str, value: &str) {
    println!("  {:<BUNDLE_LABEL_WIDTH$} {value}", format!("{label}:"));
}

/// Prints a value that continues the previous field, aligned below it.
fn continuation(value: &str) {
    println!("  {:<BUNDLE_LABEL_WIDTH$} {value}", "");
}

fn default_attestation_source(package: &PackageSource) -> PackageSource {
    match package {
        PackageSource::Url(url) => PackageSource::Url(
            mutable_sidecar_url(url).expect("a package URL always has an archive filename"),
        ),
        PackageSource::Path(path) => {
            let mut sidecar = OsString::from(path.as_os_str());
            sidecar.push(".sigs");
            PackageSource::Path(sidecar.into())
        }
    }
}

fn source_url(source: &PackageSource) -> miette::Result<Url> {
    match source {
        PackageSource::Url(url) => Ok(url.clone()),
        PackageSource::Path(path) => {
            let absolute = std::path::absolute(path)
                .into_diagnostic()
                .with_context(|| format!("failed to resolve {}", path.display()))?;
            Url::from_file_path(&absolute).map_err(|()| {
                miette::miette!("cannot convert {} to a file URL", absolute.display())
            })
        }
    }
}

async fn load_package_record(
    source: &PackageSource,
    client: &ClientWithMiddleware,
) -> miette::Result<RepoDataRecord> {
    match source {
        PackageSource::Path(path) => package_record_from_path(path).await,
        PackageSource::Url(url) => {
            let download_dir = tempfile::tempdir()
                .into_diagnostic()
                .context("failed to create temporary download directory")?;
            let filename = DistArchiveIdentifier::try_from_url(url)
                .ok_or_else(|| miette::miette!("could not derive a package identity from URL"))?
                .to_file_name();
            let package_path = download_dir.path().join(filename);
            download_package(client, url, &package_path).await?;
            let mut record = package_record_from_path(&package_path).await?;
            record.url = url.clone();
            Ok(record)
        }
    }
}

async fn package_record_from_path(path: &Path) -> miette::Result<RepoDataRecord> {
    repodata_record_from_package_archive(path)
        .await
        .into_diagnostic()
        .with_context(|| format!("failed to read package metadata from {}", path.display()))
}

async fn download_package(
    client: &ClientWithMiddleware,
    url: &Url,
    destination: &Path,
) -> miette::Result<()> {
    let display_url = url.clone().redact();
    let response = client
        .get(url.clone())
        .send()
        .await
        .map_err(Redact::redact)
        .into_diagnostic()
        .with_context(|| format!("failed to download {display_url}"))?
        .error_for_status()
        .map_err(Redact::redact)
        .into_diagnostic()
        .with_context(|| format!("server returned an error for {display_url}"))?;

    let mut file = tokio::fs::File::create(destination)
        .await
        .into_diagnostic()
        .with_context(|| format!("failed to create {}", destination.display()))?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(Redact::redact)
            .into_diagnostic()
            .with_context(|| format!("failed to read package from {display_url}"))?;
        file.write_all(&chunk)
            .await
            .into_diagnostic()
            .with_context(|| format!("failed to write {}", destination.display()))?;
    }
    file.flush()
        .await
        .into_diagnostic()
        .with_context(|| format!("failed to flush {}", destination.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_remote_attestation_appends_sigs_before_the_query() {
        let package = PackageSource::Url(
            Url::parse("https://example.com/noarch/foo-1.0-0.conda?token=abc").unwrap(),
        );
        let PackageSource::Url(sidecar) = default_attestation_source(&package) else {
            panic!("expected a URL")
        };
        assert_eq!(sidecar.path(), "/noarch/foo-1.0-0.conda.sigs");
        assert_eq!(sidecar.query(), Some("token=abc"));
    }

    #[test]
    fn build_config_is_shortened_to_the_path_in_the_repository() {
        let mut claims = CertificateClaims::default();
        claims.source_repository_uri = Some("https://github.com/org/repo".to_string());
        claims.source_repository_ref = Some("refs/heads/main".to_string());
        assert_eq!(
            shorten_build_config(
                "https://github.com/org/repo/.github/workflows/publish.yml@refs/heads/main",
                &claims
            ),
            ".github/workflows/publish.yml"
        );

        // A build config in another repository, as used by a reusable workflow,
        // must stay recognizable.
        assert_eq!(
            shorten_build_config(
                "https://github.com/other/repo/.github/workflows/shared.yml@refs/heads/main",
                &claims
            ),
            "https://github.com/other/repo/.github/workflows/shared.yml"
        );
    }

    #[test]
    fn only_the_public_good_log_gets_a_link() {
        assert_eq!(
            transparency_log_url(Some("rekor.sigstore.dev - 1193050959916656506"), 42).as_deref(),
            Some("https://search.sigstore.dev/?logIndex=42")
        );
        assert_eq!(transparency_log_url(Some("rekor.example.com"), 42), None);
        assert_eq!(transparency_log_url(None, 42), None);
    }

    #[test]
    fn checks_are_named_without_overstating_them() {
        let mut checks = VerifiedChecks::default();
        assert_eq!(describe_checks(&checks), "signature only");

        checks.certificate_chain = true;
        checks.signed_certificate_timestamp = true;
        checks.transparency_log = true;
        assert_eq!(
            describe_checks(&checks),
            "certificate chain, SCT, log inclusion promise"
        );

        checks.inclusion_proof = true;
        assert_eq!(
            describe_checks(&checks),
            "certificate chain, SCT, log inclusion proof"
        );
    }

    #[test]
    fn default_local_attestation_appends_sigs() {
        let package = PackageSource::Path("foo-1.0-0.conda".into());
        let PackageSource::Path(sidecar) = default_attestation_source(&package) else {
            panic!("expected a path")
        };
        assert_eq!(sidecar, std::path::PathBuf::from("foo-1.0-0.conda.sigs"));
    }
}
