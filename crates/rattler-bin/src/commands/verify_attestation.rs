//! Verify a Sigstore attestation sidecar against a conda package archive.

use std::{ffi::OsString, path::Path};

use console::style;
use futures_util::StreamExt;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{RepoDataRecord, package::DistArchiveIdentifier};
use rattler_package_streaming::fs::repodata_record_from_package_archive;
use rattler_redaction::Redact;
use rattler_sigstore::{
    CONDA_PUBLISH_PREDICATE_TYPE, ChannelCheck, DEFAULT_MAX_SIDECAR_SIZE, FulcioCiClaims,
    RejectedAttestation, SigstoreError, VerifiedAttestation, VerifiedChecks, embedded_trusted_root,
    fetch_bundles, mutable_sidecar_url, production_trusted_root, verify_bundles,
};
use reqwest_middleware::ClientWithMiddleware;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use url::Url;

use super::{
    client::create_client_with_middleware,
    hyperlink::{self, Stream},
    package_source::PackageSource,
};
use crate::publisher_args::PublisherArgs;

/// A GitHub workflow's file page, pinned to its build-config revision.
fn workflow_url(uri: &str, claims: &ClaimsReport) -> Option<Url> {
    let (base, reference) = uri.rsplit_once('@').unwrap_or((uri, ""));
    let url = hyperlink::web(&Url::parse(base).ok()?)?;
    let slug = github_slug(&url)?;
    let path = url.path().strip_prefix(&format!("/{slug}/"))?;
    let revision = claims.build_config_digest.as_deref().unwrap_or_else(|| {
        reference
            .strip_prefix("refs/heads/")
            .or_else(|| reference.strip_prefix("refs/tags/"))
            .unwrap_or(reference)
    });
    if path.is_empty() || revision.is_empty() {
        return None;
    }
    Url::parse(&format!("https://github.com/{slug}/blob/{revision}/{path}")).ok()
}

fn github_slug(url: &Url) -> Option<String> {
    if url.host_str()? != "github.com" {
        return None;
    }
    let mut segments = url.path_segments()?;
    let owner = segments.next().filter(|s| !s.is_empty())?;
    let repo = segments.next().filter(|s| !s.is_empty())?;
    Some(format!("{owner}/{repo}"))
}

/// Compact labels are used only when the full destination is clickable.
fn compact_link(url: Option<Url>, label: &str, fallback: &str) -> String {
    if url.is_some() && hyperlink::enabled(Stream::Stdout) {
        hyperlink::maybe_link(url, label)
    } else {
        fallback.to_string()
    }
}

fn run_label(url: &Url) -> Option<String> {
    github_slug(url)?;
    let segments: Vec<_> = url.path_segments()?.collect();
    match segments.as_slice() {
        [_, _, "actions", "runs", id] => Some(format!("run {id}")),
        [_, _, "actions", "runs", id, "attempts", attempt] => {
            Some(format!("run {id}, attempt {attempt}"))
        }
        _ => None,
    }
}

fn sidecar_label(url: &Url) -> String {
    let name = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .unwrap_or(url.as_str());
    match name.rsplit_once('.') {
        Some((head, digest))
            if digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            format!("{head}.{}…", &digest[..10])
        }
        _ => name.to_string(),
    }
}

/// The sidecar `url` itself, if it can be opened from a terminal.
///
/// Unlike the URLs that come out of the attestation, this one is the sidecar the
/// user named on the command line, so a local one is linked as well: a `file://`
/// URL here only ever points at the path they passed in.
fn attestation_link(url: &str) -> Option<Url> {
    let url = Url::parse(url).ok()?;
    matches!(url.scheme(), "http" | "https" | "file").then_some(url)
}

/// The page showing `commit` in `repository`, for the forges whose URL for it
/// can be derived from the repository URL.
///
/// A commit digest is the value a trust policy is pinned to, so being able to
/// open the commit it names is worth the host specific knowledge; an unknown
/// forge gets no link rather than a guessed one.
fn commit_url(repository: Option<&str>, commit: &str) -> Option<Url> {
    let repository = repository?;
    let path = match Url::parse(repository).ok()?.host_str()? {
        // Gitea and Forgejo, which Codeberg runs, use the same path as GitHub.
        "github.com" | "codeberg.org" => format!("{repository}/commit/{commit}"),
        "gitlab.com" => format!("{repository}/-/commit/{commit}"),
        _ => return None,
    };
    hyperlink::web(&Url::parse(&path).ok()?)
}

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
    not_after: String,
    claims: ClaimsReport,
}

/// The CI claims of a signing certificate, as JSON.
///
/// This mirrors [`FulcioCiClaims`] rather than serializing it, because the
/// upstream type is deliberately free of serde and may grow fields: the JSON
/// this command emits is a contract with its callers, so it is spelled out here
/// where a change to it is visible in review. Every key is always present, an
/// absent claim as `null`, so consumers see a stable set of fields.
#[derive(Debug, Default, Serialize)]
struct ClaimsReport {
    build_signer_uri: Option<String>,
    build_signer_digest: Option<String>,
    runner_environment: Option<String>,
    source_repository_uri: Option<String>,
    source_repository_digest: Option<String>,
    source_repository_ref: Option<String>,
    source_repository_identifier: Option<String>,
    source_repository_owner_uri: Option<String>,
    source_repository_owner_identifier: Option<String>,
    build_config_uri: Option<String>,
    build_config_digest: Option<String>,
    build_trigger: Option<String>,
    run_invocation_uri: Option<String>,
    source_repository_visibility_at_signing: Option<String>,
    deployment_environment: Option<String>,
    token_subject: Option<String>,
}

impl From<&FulcioCiClaims> for ClaimsReport {
    fn from(claims: &FulcioCiClaims) -> Self {
        Self {
            build_signer_uri: claims.build_signer_uri.clone(),
            build_signer_digest: claims.build_signer_digest.clone(),
            runner_environment: claims.runner_environment.clone(),
            source_repository_uri: claims.source_repository_uri.clone(),
            source_repository_digest: claims.source_repository_digest.clone(),
            source_repository_ref: claims.source_repository_ref.clone(),
            source_repository_identifier: claims.source_repository_identifier.clone(),
            source_repository_owner_uri: claims.source_repository_owner_uri.clone(),
            source_repository_owner_identifier: claims.source_repository_owner_identifier.clone(),
            build_config_uri: claims.build_config_uri.clone(),
            build_config_digest: claims.build_config_digest.clone(),
            build_trigger: claims.build_trigger.clone(),
            run_invocation_uri: claims.run_invocation_uri.clone(),
            source_repository_visibility_at_signing: claims
                .source_repository_visibility_at_signing
                .clone(),
            deployment_environment: claims.deployment_environment.clone(),
            token_subject: claims.token_subject.clone(),
        }
    }
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
                    not_before: certificate.not_before.to_string(),
                    not_after: certificate.not_after.to_string(),
                    claims: (&certificate.ci_claims).into(),
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
        let log_index = entry.log_index.get();
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
    field(
        "Attestation",
        &compact_link(
            attestation_link(&report.attestation_url),
            &Url::parse(&report.attestation_url)
                .map(|url| sidecar_label(&url))
                .unwrap_or_else(|_| report.attestation_url.clone()),
            &report.attestation_url,
        ),
    );

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
    let identity = bundle.identity.as_deref().unwrap_or(UNKNOWN);
    // A certificate identity is a literal policy value, not a web page.
    indented("Identity", identity);
    indented("Issuer", bundle.issuer.as_deref().unwrap_or(UNKNOWN));

    if let Some(certificate) = &bundle.certificate {
        let claims = &certificate.claims;
        // The identity above is what the provider derived for the certificate;
        // this is what the token it was requested with actually claimed, which
        // is the value a provider's own documentation describes.
        if let Some(subject) = &claims.token_subject {
            indented("Token subject", subject);
        }
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
            // Only the repository URL is linked, not the annotations that
            // follow it, so that what the link covers is what it points at.
            let link = compact_link(
                Url::parse(repository)
                    .ok()
                    .as_ref()
                    .and_then(hyperlink::web),
                &Url::parse(repository)
                    .ok()
                    .and_then(|url| github_slug(&url))
                    .unwrap_or_else(|| repository.clone()),
                repository,
            );
            if annotations.is_empty() {
                indented("Repository", &link);
            } else {
                indented(
                    "Repository",
                    &format!("{link} ({})", annotations.join(", ")),
                );
            }
        }
        if let Some(commit) = &claims.source_repository_digest {
            let link = hyperlink::maybe_link(
                commit_url(claims.source_repository_uri.as_deref(), commit),
                commit,
            );
            let reference = claims.source_repository_ref.as_deref();
            indented(
                "Commit",
                &reference.map_or_else(
                    || link.clone(),
                    |reference| format!("{link} on {reference}"),
                ),
            );
        }
        if let Some(uri) = &claims.build_config_uri {
            // The text is shortened to the path in the repository, so the link
            // is what restores the full URI the claim carried.
            let workflow = compact_link(
                workflow_url(uri, claims),
                &shorten_build_config(uri, claims),
                uri,
            );
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
        // The deployment environment a job ran in is what deployment protection
        // rules hang off, so it says more about how guarded the build was than
        // the ref does. It is absent for a job that declared no environment.
        if let Some(environment) = &claims.deployment_environment {
            indented("Environment", environment);
        }
        if let Some(run) = &claims.run_invocation_uri {
            indented(
                "Build",
                &compact_link(
                    Url::parse(run).ok().as_ref().and_then(hyperlink::web),
                    &Url::parse(run)
                        .ok()
                        .as_ref()
                        .and_then(run_label)
                        .unwrap_or_else(|| run.clone()),
                    run,
                ),
            );
        }
        indented("Signed at", &certificate.not_before);
    }

    if let Some(log) = &bundle.transparency_log {
        let origin = log
            .inclusion_proof
            .as_ref()
            .and_then(|proof| proof.origin.as_deref());
        let entry = origin.map_or_else(
            || format!("index {}", log.log_index),
            |origin| format!("index {} on {}", log.log_index, log_host(origin)),
        );
        let url = log
            .url
            .as_deref()
            .and_then(|url| Url::parse(url).ok())
            .as_ref()
            .and_then(hyperlink::web);
        let linked = url.is_some() && hyperlink::enabled(Stream::Stdout);
        indented("Transparency log", &hyperlink::maybe_link(url, &entry));
        // The URL only needs a line of its own where the entry above it is not
        // already clickable.
        if let Some(url) = &log.url
            && !linked
        {
            continuation(url);
        }
    }

    let channel = bundle.target_channel.as_deref().unwrap_or("<none>");
    indented(
        "Target channel",
        &hyperlink::maybe_link(
            hyperlink::channel_page(channel)
                .or_else(|| Url::parse(channel).ok().as_ref().and_then(hyperlink::web)),
            channel,
        ),
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
fn shorten_build_config(build_config_uri: &str, claims: &ClaimsReport) -> String {
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
    fn workflow_links_use_the_build_config_revision() {
        let mut claims = ClaimsReport::default();
        let uri = "https://github.com/org/repo/.github/workflows/publish.yml@refs/heads/main";
        assert_eq!(
            workflow_url(uri, &claims).unwrap().as_str(),
            "https://github.com/org/repo/blob/main/.github/workflows/publish.yml"
        );
        claims.build_config_digest = Some("abc123".into());
        claims.source_repository_digest = Some("different-source-commit".into());
        assert_eq!(
            workflow_url(uri, &claims).unwrap().as_str(),
            "https://github.com/org/repo/blob/abc123/.github/workflows/publish.yml"
        );
        assert!(workflow_url("https://example.com/org/repo/workflow@main", &claims).is_none());
        assert!(workflow_url("someone@example.com", &claims).is_none());
    }

    #[test]
    fn compact_attestation_labels() {
        let run = Url::parse("https://github.com/org/repo/actions/runs/123/attempts/2").unwrap();
        assert_eq!(run_label(&run).as_deref(), Some("run 123, attempt 2"));
        assert!(run_label(&Url::parse("https://example.com/actions/runs/123").unwrap()).is_none());
        let sidecar = Url::parse(&format!(
            "https://example.com/pkg.conda.sigs.{}",
            "a".repeat(64)
        ))
        .unwrap();
        assert_eq!(sidecar_label(&sidecar), "pkg.conda.sigs.aaaaaaaaaa…");
    }

    #[test]
    fn commits_are_linked_only_on_forges_with_a_known_path() {
        let commit = "8a96d273f7245383c451499fb65375ec408ca042";
        assert_eq!(
            commit_url(Some("https://github.com/org/repo"), commit).map(Url::into),
            Some(format!("https://github.com/org/repo/commit/{commit}"))
        );
        assert_eq!(
            commit_url(Some("https://gitlab.com/org/repo"), commit).map(Url::into),
            Some(format!("https://gitlab.com/org/repo/-/commit/{commit}"))
        );
        assert_eq!(
            commit_url(Some("https://git.example.com/org/repo"), commit),
            None
        );
        assert_eq!(commit_url(None, commit), None);
    }

    #[test]
    fn build_config_is_shortened_to_the_path_in_the_repository() {
        let claims = ClaimsReport {
            source_repository_uri: Some("https://github.com/org/repo".to_string()),
            source_repository_ref: Some("refs/heads/main".to_string()),
            ..Default::default()
        };
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
