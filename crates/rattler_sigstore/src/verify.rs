//! Sigstore signature verification implementation.

use crate::error::{SigstoreError, SigstoreResult};
use crate::policy::{Identity, Issuer, Publisher, VerificationPolicy};
use chrono::{DateTime, Utc};
use reqwest_middleware::ClientWithMiddleware;
use sigstore_trust_root::TrustedRoot;
use sigstore_verify::{types::Bundle, VerificationPolicy as SigstorePolicy, Verifier};
use url::Url;

/// The outcome of a verification attempt.
#[derive(Debug, Clone)]
pub struct VerificationOutcome {
    /// Whether at least one signature was successfully verified.
    pub verified: bool,
    /// The identity from the verified signature certificate.
    pub identity: Option<Identity>,
    /// The issuer from the verified signature certificate.
    pub issuer: Option<Issuer>,
    /// Timestamp when the signature was created (from transparency log).
    pub signed_at: Option<DateTime<Utc>>,
    /// Any warnings generated during verification.
    pub warnings: Vec<String>,
}

impl Default for VerificationOutcome {
    fn default() -> Self {
        Self {
            verified: false,
            identity: None,
            issuer: None,
            signed_at: None,
            warnings: Vec::new(),
        }
    }
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
    verify_package_impl(policy, package_url, package_bytes, client).await
}

/// Verify a package using pre-computed SHA256 digest instead of full bytes.
///
/// This is more efficient for large packages where we already have the digest.
/// The digest should be a hex-encoded SHA256 hash string.
pub async fn verify_package_by_digest(
    policy: &VerificationPolicy,
    package_url: &Url,
    package_sha256: &str,
    client: &ClientWithMiddleware,
) -> SigstoreResult<VerificationOutcome> {
    let artifact_bytes = hex::decode(package_sha256).map_err(|e| {
        SigstoreError::VerificationFailed(format!("Invalid SHA256 hex string: {e}"))
    })?;

    verify_package_impl(policy, package_url, &artifact_bytes, client).await
}

/// Internal implementation of package verification.
async fn verify_package_impl(
    policy: &VerificationPolicy,
    package_url: &Url,
    artifact: &[u8],
    client: &ClientWithMiddleware,
) -> SigstoreResult<VerificationOutcome> {
    let config = match policy {
        VerificationPolicy::Disabled => {
            return Ok(VerificationOutcome::default());
        }
        VerificationPolicy::Warn(config) | VerificationPolicy::Require(config) => config,
    };

    // Check if we have publishers configured for this channel
    let publishers = config.publishers_for_url(package_url);
    if publishers.is_none() && policy.is_required() {
        return Err(SigstoreError::NoMatchingPublisher {
            channel: package_url.to_string(),
        });
    }

    // Fetch signatures
    let signatures_url = config.signatures_url(package_url);
    let bundles =
        fetch_signatures_with_policy(client, &signatures_url, policy, package_url).await?;

    // Handle empty bundles case
    if bundles.is_empty() {
        return handle_no_signatures(policy, package_url);
    }

    // Load trusted root and verify
    let trusted_root =
        TrustedRoot::production().map_err(|e| SigstoreError::TrustedRoot(e.to_string()))?;
    let verifier = Verifier::new(&trusted_root);

    verify_bundles(
        &verifier,
        &bundles,
        artifact,
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
    artifact: &[u8],
    publishers: Option<&[Publisher]>,
    policy: &VerificationPolicy,
    package_url: &Url,
) -> SigstoreResult<VerificationOutcome> {
    let mut verification_errors = Vec::new();

    for (i, bundle) in bundles.iter().enumerate() {
        let sigstore_policy = SigstorePolicy::default();

        match verifier.verify(artifact, bundle, &sigstore_policy) {
            Ok(result) if result.success => {
                let identity = result.identity.as_deref();
                let issuer = result.issuer.as_deref();

                let matches_publisher = match publishers {
                    Some(pubs) if !pubs.is_empty() => {
                        pubs.iter().any(|p| p.matches(identity, issuer))
                    }
                    _ => true, // No publisher constraints
                };

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
                            .and_then(|ts| DateTime::from_timestamp(ts, 0)),
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
