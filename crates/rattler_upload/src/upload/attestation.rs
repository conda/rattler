use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use miette::IntoDiagnostic;
use reqwest::header;
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;

use crate::upload::package::sha256_sum;

/// Attestation bundle structure based on in-toto attestation format
#[derive(Debug, Serialize, Deserialize)]
pub struct AttestationBundle {
    #[serde(rename = "_type")]
    pub bundle_type: String,
    pub spec_version: String,
    pub attestations: Vec<Attestation>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Attestation {
    pub envelope: Envelope,
    pub verification_material: VerificationMaterial,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub payload: String,
    #[serde(rename = "payloadType")]
    pub payload_type: String,
    pub signatures: Vec<Signature>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Signature {
    pub sig: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VerificationMaterial {
    pub x509_certificate_chain: Option<X509CertificateChain>,
    pub transparency_entries: Vec<TransparencyEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct X509CertificateChain {
    pub certificates: Vec<Certificate>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Certificate {
    pub raw_bytes: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TransparencyEntry {
    pub inclusion_proof: InclusionProof,
    pub log_index: String,
    pub log_id: LogId,
    pub kind_version: KindVersion,
    pub canonicalized_body: String,
    pub integrated_time: String,
    pub inclusion_promise: Option<InclusionPromise>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InclusionProof {
    pub checkpoint: String,
    pub hashes: Vec<String>,
    pub log_index: String,
    pub root_hash: String,
    pub tree_size: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LogId {
    pub key_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct KindVersion {
    pub kind: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InclusionPromise {
    pub signed_entry_timestamp: String,
}

/// In-toto Statement structure
#[derive(Debug, Serialize, Deserialize)]
pub struct Statement {
    #[serde(rename = "_type")]
    pub statement_type: String,
    pub subject: Vec<Subject>,
    #[serde(rename = "predicateType")]
    pub predicate_type: String,
    pub predicate: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Subject {
    pub name: String,
    pub digest: DigestSet,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DigestSet {
    pub sha256: String,
}

/// Conda-specific predicate for attestations
#[derive(Debug, Serialize, Deserialize)]
pub struct CondaPredicate {
    #[serde(rename = "targetChannel")]
    pub target_channel: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_info: Option<serde_json::Value>,
}

/// Create an attestation for a conda package
pub async fn create_conda_attestation(
    package_path: &Path,
    channel_url: &str,
    oidc_token: &str,
    client: &ClientWithMiddleware,
) -> miette::Result<AttestationBundle> {
    // Calculate package hash
    let package_hash = sha256_sum(package_path).into_diagnostic()?;
    let package_name = package_path
        .file_name()
        .ok_or_else(|| miette::miette!("Package path has no filename"))?
        .to_string_lossy()
        .to_string();

    // Create the in-toto statement
    let statement = Statement {
        statement_type: "https://in-toto.io/Statement/v1".to_string(),
        subject: vec![Subject {
            name: package_name.clone(),
            digest: DigestSet {
                sha256: package_hash.clone(),
            },
        }],
        predicate_type: "https://schemas.conda.org/attestations-publish-1.schema.json".to_string(),
        predicate: json!({
            "targetChannel": channel_url,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }),
    };

    // Serialize statement to JSON and base64 encode
    let statement_json = serde_json::to_vec(&statement).into_diagnostic()?;
    let payload = BASE64.encode(&statement_json);

    // Sign the attestation using GitHub's attestation service
    let attestation =
        sign_attestation_with_github(&payload, "application/vnd.in-toto+json", oidc_token, client)
            .await?;

    Ok(attestation)
}

/// Sign an attestation using GitHub's attestation service
async fn sign_attestation_with_github(
    payload: &str,
    payload_type: &str,
    oidc_token: &str,
    client: &ClientWithMiddleware,
) -> miette::Result<AttestationBundle> {
    // GitHub's attestation API endpoint
    // Note: This endpoint requires the OIDC token to have the correct audience and permissions
    let attestation_url = "https://api.github.com/attestations";

    // Create the request body following GitHub's attestation API format
    let request_body = json!({
        "bundle": {
            "dsseEnvelope": {
                "payload": payload,
                "payloadType": payload_type,
            }
        }
    });

    tracing::debug!("Creating attestation via GitHub API");

    // Make the request to GitHub's attestation API
    let response = client
        .post(attestation_url)
        .header(header::AUTHORIZATION, format!("Bearer {}", oidc_token))
        .header(header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .json(&request_body)
        .send()
        .await
        .into_diagnostic()?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.into_diagnostic()?;

        // Provide helpful error messages based on status code
        let error_detail = match status.as_u16() {
            401 => "Authentication failed. The OIDC token may be invalid or expired.",
            403 => "Permission denied. Ensure the workflow has 'attestations: write' permission.",
            404 => {
                "Attestation API not found. This may not be available in your GitHub environment."
            }
            422 => "Invalid request format. Check the attestation payload structure.",
            _ => "Unknown error occurred while creating attestation.",
        };

        return Err(miette::miette!(
            "{}\nStatus: {}\nResponse: {}",
            error_detail,
            status,
            body
        ));
    }

    let bundle: AttestationBundle = response.json().await.into_diagnostic()?;
    tracing::info!("Successfully created attestation via GitHub API");
    Ok(bundle)
}

// Create an attestation using Sigstore directly (for public repositories)
// pub async fn create_sigstore_attestation(
//     package_path: &Path,
//     channel_url: &str,
//     oidc_token: &str,
//     client: &ClientWithMiddleware,
// ) -> miette::Result<AttestationBundle> {
//     // This would use Sigstore's public-good instance for signing
//     // Implementation would involve:
//     // 1. Get a signing certificate from Fulcio
//     // 2. Sign the statement
//     // 3. Upload to Rekor transparency log
//     // 4. Create the attestation bundle

//     // For now, we'll use GitHub's attestation service
//     create_conda_attestation(package_path, channel_url, oidc_token, client).await
// }
