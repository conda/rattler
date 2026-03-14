//! Support for fetching conda packages from GitHub Actions artifacts.
//!
//! This module provides functionality to resolve `gha://` URLs to local
//! `.conda` or `.tar.bz2` package files by downloading and extracting
//! GitHub Actions artifacts.
//!
//! ## URL Format
//!
//! ```text
//! gha://owner/repo/runs/<run-id>/<artifact-name>
//! ```
//!
//! The artifact name should match the conda package filename
//! (e.g., `my-package-1.0.0-h1234_0.conda`).
//!
//! ## Authentication
//!
//! GitHub tokens are resolved in this order:
//! 1. `GITHUB_TOKEN` environment variable
//! 2. `GH_TOKEN` environment variable
//! 3. `gh` CLI config file (`~/.config/gh/hosts.yml`)

use std::io::Cursor;
use std::path::{Path, PathBuf};

use reqwest_middleware::ClientWithMiddleware;
use url::Url;

/// Errors that can occur when fetching a GitHub Actions artifact.
#[derive(Debug, thiserror::Error)]
pub enum GhaError {
    /// The `gha://` URL could not be parsed.
    #[error("invalid gha:// URL '{0}': expected gha://owner/repo/runs/<run-id>/<artifact-name>")]
    InvalidUrl(String),

    /// The run ID in the URL is not a valid integer.
    #[error("invalid run ID '{0}': expected an integer")]
    InvalidRunId(String),

    /// Failed to resolve a GitHub token.
    #[error("no GitHub token found. Set GITHUB_TOKEN or GH_TOKEN environment variable")]
    NoToken,

    /// HTTP request failed.
    #[error("GitHub API request failed: {0}")]
    Request(#[from] reqwest_middleware::Error),

    /// GitHub API returned an error status.
    #[error("GitHub API returned {status}: {body}")]
    ApiError {
        /// The HTTP status code.
        status: u16,
        /// The response body.
        body: String,
    },

    /// The artifact was not found in the run.
    #[error("artifact '{artifact_name}' not found in run {run_id}")]
    ArtifactNotFound {
        /// The run ID.
        run_id: u64,
        /// The artifact name.
        artifact_name: String,
    },

    /// Failed to download or extract the artifact zip.
    #[error("failed to extract artifact zip: {0}")]
    ZipExtract(String),

    /// No conda package found inside the artifact zip.
    #[error("no .conda or .tar.bz2 package found in artifact '{0}'")]
    NoCondaPackage(String),

    /// I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Parsed components of a `gha://` URL.
#[derive(Debug, Clone)]
pub struct GhaUrl {
    /// The repository owner.
    pub owner: String,
    /// The repository name.
    pub repo: String,
    /// The workflow run ID.
    pub run_id: u64,
    /// The artifact name.
    pub artifact_name: String,
}

impl GhaUrl {
    /// Parse a `gha://` URL.
    pub fn parse(url: &Url) -> Result<Self, GhaError> {
        if url.scheme() != "gha" {
            return Err(GhaError::InvalidUrl(url.to_string()));
        }

        let owner = url
            .host_str()
            .ok_or_else(|| GhaError::InvalidUrl(url.to_string()))?
            .to_string();

        let segments: Vec<&str> = url
            .path_segments()
            .ok_or_else(|| GhaError::InvalidUrl(url.to_string()))?
            .filter(|s| !s.is_empty())
            .collect();

        // Expected: repo/runs/<run-id>/<artifact-name>
        if segments.len() < 4 || segments[1] != "runs" {
            return Err(GhaError::InvalidUrl(url.to_string()));
        }

        let repo = segments[0].to_string();
        let run_id = segments[2]
            .parse::<u64>()
            .map_err(|_e| GhaError::InvalidRunId(segments[2].to_string()))?;
        let artifact_name = segments[3..].join("/");

        Ok(Self {
            owner,
            repo,
            run_id,
            artifact_name,
        })
    }
}

/// Returns `true` if the URL uses the `gha://` scheme.
pub fn is_gha_url(url: &Url) -> bool {
    url.scheme() == "gha"
}

/// Resolve a GitHub token from environment variables or `gh` CLI config.
fn resolve_github_token() -> Result<String, GhaError> {
    // 1. GITHUB_TOKEN env var
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            return Ok(token);
        }
    }

    // 2. GH_TOKEN env var
    if let Ok(token) = std::env::var("GH_TOKEN") {
        if !token.is_empty() {
            return Ok(token);
        }
    }

    // 3. gh CLI config file
    if let Some(token) = read_gh_cli_token() {
        return Ok(token);
    }

    Err(GhaError::NoToken)
}

/// Read the GitHub token from the `gh` CLI config file.
fn read_gh_cli_token() -> Option<String> {
    let config_path = gh_config_path()?;
    let contents = std::fs::read_to_string(config_path).ok()?;

    // Parse the hosts.yml file. It's a simple YAML structure:
    // github.com:
    //     oauth_token: gho_xxxx
    //     user: ...
    // We do a simple line-based parse to avoid a YAML dependency.
    let mut in_github_section = false;
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed == "github.com:" || trimmed == "\"github.com\":" {
            in_github_section = true;
            continue;
        }
        // New top-level key (not indented) means we left the github.com section
        if in_github_section && !line.starts_with(' ') && !line.starts_with('\t') {
            break;
        }
        if in_github_section {
            if let Some(token) = trimmed
                .strip_prefix("oauth_token:")
                .or_else(|| trimmed.strip_prefix("oauth_token :"))
            {
                let token = token.trim().trim_matches('"').trim_matches('\'');
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }
    None
}

/// Get the path to the `gh` CLI hosts config file.
fn gh_config_path() -> Option<PathBuf> {
    // GH_CONFIG_DIR takes precedence
    if let Ok(dir) = std::env::var("GH_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("hosts.yml"));
    }

    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA")
            .ok()
            .map(|appdata| PathBuf::from(appdata).join("GitHub CLI").join("hosts.yml"))
    }

    #[cfg(not(target_os = "windows"))]
    {
        // XDG_CONFIG_HOME or ~/.config
        let config_dir = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".config")))?;
        Some(config_dir.join("gh").join("hosts.yml"))
    }
}

/// Fetch a GitHub Actions artifact and extract the conda package(s) inside.
///
/// Returns the path to the extracted `.conda` or `.tar.bz2` file.
pub async fn fetch_gha_package(
    client: &ClientWithMiddleware,
    gha_url: &GhaUrl,
    cache_dir: &Path,
) -> Result<PathBuf, GhaError> {
    let token = resolve_github_token()?;

    // Check if we already have the package cached
    let artifact_cache_dir = cache_dir
        .join("gha")
        .join(&gha_url.owner)
        .join(&gha_url.repo)
        .join(gha_url.run_id.to_string())
        .join(&gha_url.artifact_name);

    if let Some(cached) = find_conda_package(&artifact_cache_dir) {
        tracing::debug!("using cached GHA artifact: {}", cached.display());
        return Ok(cached);
    }

    // Find the artifact ID via the GitHub API
    let artifact_id =
        find_artifact_id(client, &token, gha_url).await?;

    // Download the artifact zip
    let zip_bytes = download_artifact(client, &token, gha_url, artifact_id).await?;

    // Extract the conda package from the zip
    std::fs::create_dir_all(&artifact_cache_dir)?;
    extract_conda_from_zip(&zip_bytes, &artifact_cache_dir)?;

    find_conda_package(&artifact_cache_dir)
        .ok_or_else(|| GhaError::NoCondaPackage(gha_url.artifact_name.clone()))
}

/// Look for an existing `.conda` or `.tar.bz2` file in a directory.
fn find_conda_package(dir: &Path) -> Option<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.ends_with(".conda") || name.ends_with(".tar.bz2") {
                return Some(path);
            }
        }
    }
    None
}

/// Find the artifact ID for a given artifact name in a workflow run.
async fn find_artifact_id(
    client: &ClientWithMiddleware,
    token: &str,
    gha_url: &GhaUrl,
) -> Result<u64, GhaError> {
    let api_url = format!(
        "https://api.github.com/repos/{}/{}/actions/runs/{}/artifacts",
        gha_url.owner, gha_url.repo, gha_url.run_id
    );

    let response = client
        .get(&api_url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "rattler")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(GhaError::ApiError { status, body });
    }

    let body: serde_json::Value = response.json().await.map_err(|e| {
        GhaError::ZipExtract(format!("failed to parse artifact list response: {e}"))
    })?;

    let artifacts = body["artifacts"]
        .as_array()
        .ok_or_else(|| GhaError::ZipExtract("unexpected API response format".to_string()))?;

    for artifact in artifacts {
        if let Some(name) = artifact["name"].as_str() {
            if name == gha_url.artifact_name {
                if let Some(id) = artifact["id"].as_u64() {
                    return Ok(id);
                }
            }
        }
    }

    Err(GhaError::ArtifactNotFound {
        run_id: gha_url.run_id,
        artifact_name: gha_url.artifact_name.clone(),
    })
}

/// Download an artifact zip from GitHub.
async fn download_artifact(
    client: &ClientWithMiddleware,
    token: &str,
    gha_url: &GhaUrl,
    artifact_id: u64,
) -> Result<Vec<u8>, GhaError> {
    let api_url = format!(
        "https://api.github.com/repos/{}/{}/actions/artifacts/{}/zip",
        gha_url.owner, gha_url.repo, artifact_id
    );

    tracing::info!(
        "downloading GHA artifact '{}' from {}/{}#{}",
        gha_url.artifact_name,
        gha_url.owner,
        gha_url.repo,
        gha_url.run_id
    );

    let response = client
        .get(&api_url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "rattler")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(GhaError::ApiError { status, body });
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| GhaError::ZipExtract(format!("failed to read response bytes: {e}")))?;

    Ok(bytes.to_vec())
}

/// Extract conda packages from a GitHub Actions artifact zip.
fn extract_conda_from_zip(zip_bytes: &[u8], dest_dir: &Path) -> Result<(), GhaError> {
    let reader = Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| GhaError::ZipExtract(format!("failed to open zip: {e}")))?;

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| GhaError::ZipExtract(format!("failed to read zip entry: {e}")))?;

        let Some(name) = file.enclosed_name() else {
            continue;
        };

        let name_str = name.to_string_lossy();
        if name_str.ends_with(".conda") || name_str.ends_with(".tar.bz2") {
            // Extract only the filename (not subdirectory paths within the zip)
            let file_name = name
                .file_name()
                .ok_or_else(|| GhaError::ZipExtract("invalid entry name".to_string()))?;

            let dest_path = dest_dir.join(file_name);
            let mut dest_file = std::fs::File::create(&dest_path)?;
            std::io::copy(&mut file, &mut dest_file)?;

            tracing::debug!("extracted {} from GHA artifact zip", dest_path.display());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gha_url() {
        let url = Url::parse("gha://conda-forge/feedstock/runs/12345/my-package-1.0-h1234_0.conda")
            .unwrap();
        let gha_url = GhaUrl::parse(&url).unwrap();
        assert_eq!(gha_url.owner, "conda-forge");
        assert_eq!(gha_url.repo, "feedstock");
        assert_eq!(gha_url.run_id, 12345);
        assert_eq!(gha_url.artifact_name, "my-package-1.0-h1234_0.conda");
    }

    #[test]
    fn test_parse_gha_url_invalid() {
        let url = Url::parse("gha://owner/repo/12345").unwrap();
        assert!(GhaUrl::parse(&url).is_err());
    }

    #[test]
    fn test_is_gha_url() {
        let gha = Url::parse("gha://owner/repo/runs/123/pkg.conda").unwrap();
        let https = Url::parse("https://example.com/pkg.conda").unwrap();
        assert!(is_gha_url(&gha));
        assert!(!is_gha_url(&https));
    }

    #[test]
    fn test_gh_config_path() {
        // Just make sure it doesn't panic
        let _ = gh_config_path();
    }
}
