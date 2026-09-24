//! Verify a Sigstore attestation sidecar against a conda package archive.

use std::{ffi::OsString, path::Path};

use console::style;
use futures_util::StreamExt;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{RepoDataRecord, package::DistArchiveIdentifier};
use rattler_package_streaming::fs::repodata_record_from_package_archive;
use rattler_redaction::Redact;
use rattler_sigstore::{
    ChannelCheck, DEFAULT_MAX_SIDECAR_SIZE, RejectedAttestation, SigstoreError, fetch_bundles,
    mutable_sidecar_url, production_trusted_root, verify_bundles,
};
use reqwest_middleware::ClientWithMiddleware;
use tokio::io::AsyncWriteExt;
use url::Url;

use super::{client::create_client_with_middleware, hyperlink, package_source::PackageSource};
use crate::publisher_args::PublisherArgs;

/// The page a signing identity refers to, if any.
///
/// A workflow identity carries the git reference it ran from after an `@`
/// (`https://github.com/org/repo/.github/workflows/build.yml@refs/heads/main`),
/// which is not part of the URL that serves the workflow.
fn identity_url(identity: &str) -> Option<Url> {
    let (url, _reference) = identity.rsplit_once('@').unwrap_or((identity, ""));
    hyperlink::web(&Url::parse(url).ok()?)
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
    let trusted_root = production_trusted_root().await.into_diagnostic()?;
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

    println!(
        "{} {}",
        style("✓").green().bold(),
        style("Sigstore attestation verified").green().bold()
    );
    println!();
    println!("Package: {package_name}");
    let sha256 = record
        .package_record
        .sha256
        .as_ref()
        .expect("successful verification requires a SHA-256 digest");
    println!("SHA-256: {}", hex::encode(sha256));

    for attestation in verification.verified {
        println!();
        println!("Bundle: {}", attestation.index);
        let identity = attestation.identity.as_deref().unwrap_or("<unknown>");
        println!(
            "Identity: {}",
            hyperlink::maybe_link(identity_url(identity), identity)
        );
        println!(
            "Issuer: {}",
            attestation.issuer.as_deref().unwrap_or("<unknown>")
        );
        println!(
            "Integrated at: {}",
            attestation
                .integrated_time
                .map_or_else(|| "<unknown>".to_string(), |time| time.to_string())
        );
        let target_channel = attestation.target_channel.as_deref().unwrap_or("<none>");
        println!(
            "Target channel: {}",
            hyperlink::maybe_link(
                Url::parse(target_channel)
                    .ok()
                    .as_ref()
                    .and_then(hyperlink::web),
                target_channel
            )
        );
        for warning in attestation.warnings {
            eprintln!("{} {warning}", style("warning:").yellow().bold());
        }
    }

    for rejected in verification.rejected {
        eprintln!(
            "{} bundle {}: {}",
            style("warning:").yellow().bold(),
            rejected.index,
            rejected.reason
        );
    }

    Ok(())
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
    fn default_local_attestation_appends_sigs() {
        let package = PackageSource::Path("foo-1.0-0.conda".into());
        let PackageSource::Path(sidecar) = default_attestation_source(&package) else {
            panic!("expected a path")
        };
        assert_eq!(sidecar, std::path::PathBuf::from("foo-1.0-0.conda.sigs"));
    }
}
