//! Sigstore signature verification implementation.

use crate::error::{SigstoreError, SigstoreResult};
use crate::policy::{Identity, Issuer, Publisher, VerificationPolicy};
use jiff::Timestamp;
use reqwest_middleware::ClientWithMiddleware;
use sigstore_verify::trust_root::TrustedRoot;
use sigstore_verify::{
    VerificationPolicy as SigstorePolicy, Verifier,
    types::{Artifact, Bundle},
};
use tokio::sync::OnceCell;
use url::Url;

/// Process-wide cache of the Sigstore production trusted root.
///
/// [`TrustedRoot::production`] performs network I/O (TUF) to fetch the root, so
/// loading it once per package verification is wasteful when installing many
/// packages. We cache the first successful load for the lifetime of the
/// process; failures are not cached so a later call can retry.
static TRUSTED_ROOT: OnceCell<TrustedRoot> = OnceCell::const_new();

/// Load the production trusted root, reusing the cached instance if available.
async fn production_trusted_root() -> SigstoreResult<&'static TrustedRoot> {
    TRUSTED_ROOT
        .get_or_try_init(|| async {
            TrustedRoot::production()
                .await
                .map_err(|e| SigstoreError::TrustedRoot(e.to_string()))
        })
        .await
}

/// The outcome of a verification attempt.
#[derive(Debug, Clone, Default)]
pub struct VerificationOutcome {
    /// Whether at least one signature was successfully verified.
    pub verified: bool,
    /// The identity from the verified signature certificate.
    pub identity: Option<Identity>,
    /// The issuer from the verified signature certificate.
    pub issuer: Option<Issuer>,
    /// Timestamp when the signature was created (from transparency log).
    pub signed_at: Option<Timestamp>,
    /// Any warnings generated during verification.
    pub warnings: Vec<String>,
}

/// Verify a package's signatures using full package bytes.
///
/// This function:
/// 1. Fetches signatures from the signatures URL (derived from package URL)
/// 2. Verifies each signature against the Sigstore trusted root
/// 3. Checks that at least one signature matches the allowed publishers
///
/// # Arguments
///
/// * `policy` - The verification policy to use
/// * `package_url` - The URL of the package being verified
/// * `package_bytes` - The raw bytes of the downloaded package
/// * `client` - HTTP client for fetching signatures
///
/// # Returns
///
/// Returns `Ok(VerificationOutcome)` if verification succeeds or is disabled,
/// or an error if verification is required and fails.
pub async fn verify_package(
    policy: &VerificationPolicy,
    package_url: &Url,
    package_bytes: &[u8],
    client: &ClientWithMiddleware,
) -> SigstoreResult<VerificationOutcome> {
    verify_package_impl(
        policy,
        package_url,
        Artifact::from_bytes(package_bytes),
        client,
    )
    .await
}

/// Verify a package using a pre-computed SHA256 digest instead of full bytes.
///
/// This is more efficient for large packages where we already have the digest.
/// `package_sha256` is the raw 32-byte SHA256 digest of the package (e.g.
/// `rattler_digest::Sha256Hash::as_slice`), avoiding a redundant hex
/// encode/decode round-trip at the call site.
pub async fn verify_package_by_digest(
    policy: &VerificationPolicy,
    package_url: &Url,
    package_sha256: &[u8],
    client: &ClientWithMiddleware,
) -> SigstoreResult<VerificationOutcome> {
    // Pass the SHA256 as a pre-computed digest so the verifier compares it
    // directly against the bundle instead of hashing it as raw artifact bytes.
    verify_package_impl(
        policy,
        package_url,
        Artifact::from_digest(package_sha256),
        client,
    )
    .await
}

/// Internal implementation of package verification.
async fn verify_package_impl(
    policy: &VerificationPolicy,
    package_url: &Url,
    artifact: Artifact<'_>,
    client: &ClientWithMiddleware,
) -> SigstoreResult<VerificationOutcome> {
    let config = match policy {
        VerificationPolicy::Disabled => {
            return Ok(VerificationOutcome::default());
        }
        VerificationPolicy::Warn(config) | VerificationPolicy::Require(config) => config,
    };

    // Determine which publishers are allowed for this channel. If the channel
    // is not mapped and no default publishers are configured we have nothing to
    // verify against: fail closed in `Require` mode and skip (do not verify) in
    // `Warn` mode, matching the documented behaviour.
    let Some(publishers) = config.publishers_for_url(package_url) else {
        if policy.is_required() {
            return Err(SigstoreError::NoMatchingPublisher {
                channel: package_url.to_string(),
            });
        }
        tracing::debug!(
            "No publishers configured for {}; skipping signature verification",
            package_url
        );
        return Ok(VerificationOutcome::default());
    };

    // Fetch signatures
    let signatures_url = config.signatures_url(package_url)?;
    let bundles =
        fetch_signatures_with_policy(client, &signatures_url, policy, package_url).await?;

    // Handle empty bundles case
    if bundles.is_empty() {
        return handle_no_signatures(policy, package_url);
    }

    // Load (or reuse the cached) trusted root and verify.
    let trusted_root = production_trusted_root().await?;
    let verifier = Verifier::new(trusted_root);

    verify_bundles(
        &verifier,
        &bundles,
        &artifact,
        publishers,
        policy,
        package_url,
    )
}

/// Fetch signatures, handling errors according to policy.
async fn fetch_signatures_with_policy(
    client: &ClientWithMiddleware,
    signatures_url: &Url,
    policy: &VerificationPolicy,
    package_url: &Url,
) -> SigstoreResult<Vec<Bundle>> {
    match fetch_signatures(client, signatures_url).await {
        Ok(bundles) => Ok(bundles),
        Err(e) => {
            if policy.is_required() {
                Err(e)
            } else {
                tracing::warn!("Failed to fetch signatures for {}: {}", package_url, e);
                Ok(Vec::new()) // Return empty, caller will handle as warning
            }
        }
    }
}

/// Handle the case when no signatures are found.
fn handle_no_signatures(
    policy: &VerificationPolicy,
    package_url: &Url,
) -> SigstoreResult<VerificationOutcome> {
    if policy.is_required() {
        Err(SigstoreError::NoSignatures(package_url.to_string()))
    } else {
        tracing::warn!("No signatures found for {}", package_url);
        Ok(VerificationOutcome {
            warnings: vec!["No signatures found".to_string()],
            ..Default::default()
        })
    }
}

/// Verify bundles against the artifact and return the outcome.
fn verify_bundles(
    verifier: &Verifier,
    bundles: &[Bundle],
    artifact: &Artifact<'_>,
    publishers: &[Publisher],
    policy: &VerificationPolicy,
    package_url: &Url,
) -> SigstoreResult<VerificationOutcome> {
    let mut verification_errors = Vec::new();

    for (i, bundle) in bundles.iter().enumerate() {
        let sigstore_policy = SigstorePolicy::default();

        match verifier.verify(artifact.clone(), bundle, &sigstore_policy) {
            Ok(result) if result.success => {
                let identity = result.identity.as_deref();
                let issuer = result.issuer.as_deref();

                // An empty publisher list (an explicitly configured channel
                // with no signer constraints) accepts any valid signature.
                let matches_publisher =
                    publishers.is_empty() || publishers.iter().any(|p| p.matches(identity, issuer));

                if matches_publisher {
                    tracing::info!(
                        "Signature {} verified successfully (identity: {:?}, issuer: {:?})",
                        i + 1,
                        identity,
                        issuer
                    );
                    return Ok(VerificationOutcome {
                        verified: true,
                        identity: result.identity.map(Identity::from),
                        issuer: result.issuer.map(Issuer::from),
                        signed_at: result
                            .integrated_time
                            .and_then(|ts| Timestamp::from_second(ts).ok()),
                        warnings: result.warnings,
                    });
                } else {
                    tracing::debug!(
                        "Signature {} valid but doesn't match required publishers",
                        i + 1
                    );
                    verification_errors.push(format!(
                        "Signature {} valid but identity/issuer doesn't match required publishers",
                        i + 1
                    ));
                }
            }
            Ok(_) => {
                verification_errors
                    .push(format!("Signature {} verification returned false", i + 1));
            }
            Err(e) => {
                tracing::debug!("Signature {} verification error: {}", i + 1, e);
                verification_errors.push(format!("Signature {}: {}", i + 1, e));
            }
        }
    }

    // No valid signatures found
    if policy.is_required() {
        Err(SigstoreError::VerificationFailed(
            verification_errors.join("; "),
        ))
    } else {
        tracing::warn!(
            "No valid signatures found for {}, but verification is not required",
            package_url
        );
        Ok(VerificationOutcome {
            warnings: verification_errors,
            ..Default::default()
        })
    }
}

/// Fetch signatures from a URL.
async fn fetch_signatures(client: &ClientWithMiddleware, url: &Url) -> SigstoreResult<Vec<Bundle>> {
    let response =
        client
            .get(url.clone())
            .send()
            .await
            .map_err(|e| SigstoreError::FetchSignatures {
                url: url.to_string(),
                message: e.to_string(),
            })?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        // No signatures file exists - this is not necessarily an error
        return Ok(Vec::new());
    }

    if !response.status().is_success() {
        return Err(SigstoreError::FetchSignatures {
            url: url.to_string(),
            message: format!("HTTP {}", response.status()),
        });
    }

    let body = response
        .text()
        .await
        .map_err(|e| SigstoreError::FetchSignatures {
            url: url.to_string(),
            message: e.to_string(),
        })?;

    // Parse as JSON array of bundles
    let bundles_json: Vec<serde_json::Value> = serde_json::from_str(&body)?;

    let mut bundles = Vec::new();
    for (i, bundle_json) in bundles_json.iter().enumerate() {
        let bundle_str = serde_json::to_string(bundle_json)?;
        let bundle = Bundle::from_json(&bundle_str).map_err(|e| SigstoreError::ParseBundle {
            index: i,
            message: e.to_string(),
        })?;
        bundles.push(bundle);
    }

    Ok(bundles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::VerificationConfig;
    use reqwest_middleware::ClientBuilder;

    fn dummy_client() -> ClientWithMiddleware {
        ClientBuilder::new(reqwest::Client::new()).build()
    }

    /// In `Warn` mode an unmapped channel with no default publishers is skipped
    /// (no verification performed, no warning emitted) rather than accepting any
    /// valid signer. This path must return before any network I/O.
    #[tokio::test]
    async fn warn_mode_skips_unmapped_channel() {
        let policy = VerificationPolicy::Warn(VerificationConfig::new());
        let url = Url::parse("https://conda.anaconda.org/conda-forge/linux-64/pkg.conda").unwrap();
        let outcome = verify_package_by_digest(&policy, &url, &[0u8; 32], &dummy_client())
            .await
            .expect("warn mode should skip, not error");
        assert!(!outcome.verified);
        assert!(outcome.warnings.is_empty());
    }

    /// In `Require` mode an unmapped channel with no default publishers fails
    /// closed. This path must return before any network I/O.
    #[tokio::test]
    async fn require_mode_errors_on_unmapped_channel() {
        let policy = VerificationPolicy::Require(VerificationConfig::new());
        let url = Url::parse("https://conda.anaconda.org/conda-forge/linux-64/pkg.conda").unwrap();
        let err = verify_package_by_digest(&policy, &url, &[0u8; 32], &dummy_client())
            .await
            .expect_err("require mode should fail closed");
        assert!(matches!(err, SigstoreError::NoMatchingPublisher { .. }));
    }
}
