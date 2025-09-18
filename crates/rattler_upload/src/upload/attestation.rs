use miette::IntoDiagnostic;
use reqwest::header;
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{io::Write as _, path::Path};
use tempfile::NamedTempFile;
use tokio::process::Command as AsyncCommand;

/// Conda V1 predicate
#[derive(Debug, Serialize, Deserialize)]
pub struct CondaV1Predicate {
    #[serde(rename = "targetChannel", skip_serializing_if = "Option::is_none")]
    pub target_channel: Option<String>,
}

/// In-toto Statement structure for conda packages
#[derive(Debug, Serialize, Deserialize)]
pub struct Statement {
    #[serde(rename = "_type")]
    pub statement_type: String,
    pub subject: Vec<Subject>,
    #[serde(rename = "predicateType")]
    pub predicate_type: String,
    pub predicate: CondaV1Predicate,
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

/// Response from GitHub attestation API
#[derive(Debug, Serialize, Deserialize)]
pub struct AttestationResponse {
    pub id: String,
}

/// Configuration for attestation creation
#[derive(Debug, Clone)]
pub struct AttestationConfig {
    pub repo_owner: Option<String>,
    pub repo_name: Option<String>,
    pub github_token: Option<String>,
    pub use_github_oidc: bool,
    /// Path to a local cosign private key for testing (optional)
    pub cosign_private_key: Option<String>,
}

/// Create and store an attestation for a conda package using cosign
///
/// This function:
/// 1. Creates an in-toto statement for the package
/// 2. Uses cosign to sign the statement with GitHub OIDC or other identity
/// 3. Optionally stores the signed attestation to GitHub's attestation API (if token provided)
///
/// Returns the attestation bundle JSON or GitHub attestation ID
pub async fn create_attestation_with_cosign(
    package_path: &Path,
    channel_url: &str,
    config: &AttestationConfig,
    client: &ClientWithMiddleware,
) -> miette::Result<String> {
    // Check if cosign is installed
    check_cosign_installed().await?;

    // Step 1: Create just the predicate data for cosign (not a full statement)
    let predicate = create_conda_intoto_statement(package_path, channel_url).await?;

    // Step 2: Sign with cosign
    let bundle_json = sign_with_cosign(predicate.path(), package_path, config).await?;

    // Step 3: Optionally store to GitHub if token is provided
    if let (Some(token), Some(owner), Some(repo)) =
        (&config.github_token, &config.repo_owner, &config.repo_name)
    {
        let attestation_id =
            store_attestation_to_github(&bundle_json, token, owner, repo, client).await?;

        tracing::info!("Attestation stored to GitHub with ID: {}", attestation_id);
        Ok(attestation_id)
    } else {
        tracing::info!("GitHub token not provided, skipping GitHub attestation storage");
        // Return the bundle JSON for use elsewhere (e.g., prefix.dev upload)
        Ok(bundle_json)
    }
}

/// Check if cosign is installed and available
async fn check_cosign_installed() -> miette::Result<()> {
    let output = AsyncCommand::new("cosign")
        .arg("version")
        .output()
        .await
        .into_diagnostic()
        .map_err(|_| {
            miette::miette!(
                "cosign is not installed or not found in PATH.\n\
             Install it with: pixi global install cosign"
            )
        })?;

    if !output.status.success() {
        return Err(miette::miette!(
            "cosign command failed. Please ensure cosign is properly installed.\n\
             Install it with: pixi global install cosign"
        ));
    }

    let version = String::from_utf8_lossy(&output.stdout);
    tracing::info!("Using cosign version: {}", version.trim());

    Ok(())
}

/// Create just the predicate data for conda package attestation
async fn create_conda_intoto_statement(
    filepath: &Path,
    channel_url: &str,
) -> miette::Result<tempfile::NamedTempFile> {
    let mut temp_file = NamedTempFile::new().into_diagnostic()?;
    let subject = Subject {
        name: filepath
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| miette::miette!("Invalid package file name"))?
            .to_string(),
        digest: {
            let digest = rattler_digest::compute_file_digest::<rattler_digest::Sha256>(filepath)
                .into_diagnostic()?;
            DigestSet {
                sha256: format!("{digest:x}"),
            }
        },
    };

    let statement = Statement {
        statement_type: "https://in-toto.io/Statement/v1".to_string(),
        subject: vec![subject],
        predicate_type: "https://schemas.conda.org/attestations-publish-1.schema.json".to_string(),
        predicate: CondaV1Predicate {
            target_channel: Some(channel_url.to_string()),
        },
    };
    temp_file
        .write_all(
            serde_json::to_string(&statement)
                .into_diagnostic()?
                .as_bytes(),
        )
        .into_diagnostic()?;
    Ok(temp_file)
}

/// Sign a predicate using cosign
async fn sign_with_cosign(
    predicate_path: &Path,
    package_path: &Path,
    config: &AttestationConfig,
) -> miette::Result<String> {
    tracing::debug!(
        "Signing predicate with cosign: {}",
        predicate_path.display()
    );

    // Build cosign attest command
    // For conda packages, we'll use cosign attest-blob since we don't have an OCI image
    let mut cmd = AsyncCommand::new("cosign");
    cmd.arg("attest-blob")
        .arg("--statement")
        .arg(predicate_path)
        .arg("--type")
        .arg("https://schemas.conda.org/attestations-publish-1.schema.json");

    // Bundle file for keyless signing (declare here so it's accessible later)
    let bundle_file: Option<NamedTempFile>;

    // Check if using local key for testing
    if let Some(key_path) = &config.cosign_private_key {
        bundle_file = None; // Not needed for local key signing

        tracing::info!("Using local cosign key for signing: {}", key_path);
        cmd.arg("--key").arg(key_path);

        // Check if password is needed
        if std::env::var("COSIGN_PASSWORD").is_err() {
            tracing::warn!(
                "No COSIGN_PASSWORD set. If your key is password-protected, set COSIGN_PASSWORD env var."
            );
        }

        // For local key signing, output the bundle to stdout and disable tlog upload
        cmd.arg("--bundle").arg("-"); // Output bundle to stdout
        cmd.arg("--tlog-upload=false"); // Don't upload to transparency log for local testing

        tracing::warn!(
            "Local key signing produces DSSE format, not Sigstore bundle format.\n\
             For prefix.dev uploads, use keyless signing (GitHub Actions) to get proper Sigstore bundles."
        );
    }
    // Configure identity provider for keyless signing
    else if config.use_github_oidc {
        // Check if we're in GitHub Actions
        let in_github_actions = std::env::var("GITHUB_ACTIONS").is_ok();

        if !in_github_actions {
            // For local testing, check if user wants to use keyless signing
            if std::env::var("COSIGN_EXPERIMENTAL").is_ok() {
                tracing::info!(
                    "Local testing mode: Using cosign keyless signing.\n\
                     This will authenticate via browser OAuth flow (Google, GitHub, Microsoft)."
                );
            } else {
                tracing::warn!(
                    "Not running in GitHub Actions. To test locally, you can:\n\
                     1. Set COSIGN_EXPERIMENTAL=1 for keyless signing via browser\n\
                     2. Use cosign generate-key-pair for local key-based signing"
                );
            }
        }

        // Use experimental keyless signing
        cmd.env("COSIGN_EXPERIMENTAL", "1");

        // For keyless signing, we need to output to a temp file to get proper Sigstore bundle format
        bundle_file = Some(NamedTempFile::new().into_diagnostic()?);
        let bundle_path = bundle_file
            .as_ref()
            .unwrap()
            .path()
            .to_string_lossy()
            .to_string();
        cmd.arg("--bundle").arg(&bundle_path);
        cmd.arg("--new-bundle-format=true"); // Use the new Sigstore bundle format (v0.3)

        // Only skip prompts in CI environments
        if in_github_actions || std::env::var("CI").is_ok() {
            cmd.env("COSIGN_YES", "true");
        }

        // Set GitHub-specific environment if available
        if let Ok(server_url) = std::env::var("GITHUB_SERVER_URL") {
            if server_url != "https://github.com" {
                // For GitHub Enterprise, set custom Fulcio URL
                let url = url::Url::parse(&server_url).into_diagnostic()?;
                let host = url
                    .host_str()
                    .ok_or_else(|| miette::miette!("Invalid GitHub server URL"))?;
                cmd.env("SIGSTORE_FULCIO_URL", format!("https://fulcio.{host}"));
                cmd.env("SIGSTORE_REKOR_URL", ""); // GitHub uses TSA, not Rekor
            }
        }
    } else {
        bundle_file = None;
    }

    // Add the blob (package file) to attest
    cmd.arg(package_path);

    tracing::info!("Running cosign to create attestation...");

    // Add timeout to prevent hanging
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), cmd.output())
        .await
        .into_diagnostic()
        .map_err(|_| miette::miette!("cosign command timed out after 30 seconds"))?
        .into_diagnostic()
        .map_err(|e| miette::miette!("Failed to run cosign: {}", e))?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    if !output.status.success() {
        return Err(miette::miette!(
            "cosign attestation failed with exit code {:?}:\n\
             stdout: {}\n\
             stderr: {}\n\n\
             Troubleshooting:\n\
             1. Ensure you're running in GitHub Actions with 'id-token: write' permission\n\
             2. Check that GITHUB_TOKEN is set if uploading to GitHub\n\
             3. For local testing, ensure you have valid credentials configured",
            output.status.code(),
            if stdout.is_empty() {
                "(empty)"
            } else {
                &stdout
            },
            if stderr.is_empty() {
                "(empty)"
            } else {
                &stderr
            }
        ));
    }

    // Get the bundle JSON - either from stdout (local key) or from bundle file (keyless)
    let bundle_json = if config.cosign_private_key.is_some() {
        // Local key signing outputs to stdout
        let result = stdout.to_string();
        if result.is_empty() {
            return Err(miette::miette!(
                "cosign produced empty output for local key signing.\n\
                 stderr output: {}",
                if stderr.is_empty() {
                    "(empty)"
                } else {
                    &stderr
                }
            ));
        }
        result
    } else if let Some(ref bundle_file) = bundle_file {
        // Keyless signing may output to both stdout and bundle file
        // Check if stdout contains the output (happens in some cosign versions)
        if !stdout.is_empty() {
            tracing::info!("Cosign output to stdout, checking format...");

            // Check if stdout is a proper Sigstore bundle or just DSSE
            if stdout.contains("\"mediaType\"") || stdout.contains("\"rekorBundle\"") {
                // This looks like a Sigstore bundle
                tracing::info!("Using Sigstore bundle from stdout");
                stdout.to_string()
            } else {
                // stdout contains DSSE, try reading the bundle file
                tracing::info!("Stdout contains DSSE format, reading bundle file instead");
                let bundle_content = std::fs::read_to_string(bundle_file.path())
                    .into_diagnostic()
                    .map_err(|e| miette::miette!("Failed to read bundle file: {}", e))?;

                if !bundle_content.is_empty() {
                    tracing::info!("Using bundle from file");
                    bundle_content
                } else {
                    // Bundle file is empty, fall back to stdout (DSSE format)
                    tracing::warn!("Bundle file is empty, using DSSE format from stdout");
                    stdout.to_string()
                }
            }
        } else {
            // No stdout, read from bundle file
            std::fs::read_to_string(bundle_file.path())
                .into_diagnostic()
                .map_err(|e| miette::miette!("Failed to read bundle file: {}", e))?
        }
    } else {
        return Err(miette::miette!(
            "Neither local key nor bundle file available for reading attestation result"
        ));
    };

    tracing::info!("Successfully created attestation with cosign");

    Ok(bundle_json)
}

/// Store a signed attestation bundle to GitHub's attestation API
async fn store_attestation_to_github(
    bundle_json: &str,
    github_token: &str,
    owner: &str,
    repo: &str,
    client: &ClientWithMiddleware,
) -> miette::Result<String> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}/attestations");

    // Parse the bundle JSON to ensure it's valid
    let bundle: serde_json::Value = serde_json::from_str(bundle_json)
        .into_diagnostic()
        .map_err(|e| miette::miette!("Invalid bundle JSON from cosign: {}", e))?;

    let request_body = json!({
        "bundle": bundle,
    });

    tracing::debug!("Storing attestation to GitHub at {}", url);

    let response = client
        .post(&url)
        .bearer_auth(github_token)
        .header(header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .json(&request_body)
        .send()
        .await
        .into_diagnostic()?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.into_diagnostic()?;

        let error_detail = match status.as_u16() {
            401 => "Authentication failed. Check your GitHub token.",
            403 => "Permission denied. Ensure the token has 'attestations:write' permission and the repository allows attestations.",
            404 => "Repository not found or attestations API not available. Ensure you're using a supported GitHub plan.",
            422 => "Invalid attestation bundle format. Check the cosign output.",
            _ => "Unknown error occurred while storing attestation.",
        };

        return Err(miette::miette!(
            "{}\nStatus: {}\nResponse: {}",
            error_detail,
            status,
            body
        ));
    }

    let response_data: AttestationResponse = response.json().await.into_diagnostic()?;
    tracing::info!(
        "Successfully stored attestation with ID: {}",
        response_data.id
    );

    Ok(response_data.id)
}

// Backward compatibility function
pub async fn create_conda_attestation(
    package_path: &Path,
    channel_url: &str,
    _oidc_token: &str,
    client: &ClientWithMiddleware,
) -> miette::Result<serde_json::Value> {
    // Try to extract repo info from environment (optional)
    let (repo_owner, repo_name) = if let Ok(repo) = std::env::var("GITHUB_REPOSITORY") {
        let parts: Vec<&str> = repo.split('/').collect();
        if parts.len() == 2 {
            (Some(parts[0].to_string()), Some(parts[1].to_string()))
        } else {
            tracing::warn!(
                "Invalid GITHUB_REPOSITORY format '{}'. Expected 'owner/repo'. \
                 GitHub attestation storage will be skipped.",
                repo
            );
            (None, None)
        }
    } else {
        tracing::info!(
            "GITHUB_REPOSITORY environment variable not set. \
             GitHub attestation storage will be skipped."
        );
        (None, None)
    };

    // Try to get GitHub token (optional)
    let github_token = std::env::var("GITHUB_TOKEN").ok();

    if github_token.is_none() {
        tracing::info!(
            "GITHUB_TOKEN environment variable not set. \
             GitHub attestation storage will be skipped."
        );
    }

    let config = AttestationConfig {
        repo_owner,
        repo_name,
        github_token,
        use_github_oidc: true,
        cosign_private_key: std::env::var("COSIGN_KEY_PATH").ok(),
    };

    let attestation_result =
        create_attestation_with_cosign(package_path, channel_url, &config, client).await?;

    serde_json::from_str(&attestation_result)
        .into_diagnostic()
        .map_err(|e| miette::miette!("Failed to parse Sigstore bundle: {}", e))
}
