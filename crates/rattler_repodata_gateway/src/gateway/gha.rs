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
//! The artifact name is matched against the GitHub Actions artifact name for
//! the given run. It supports a `{subdir}` placeholder that is substituted
//! with the current platform's conda subdirectory (e.g. `linux-64`,
//! `osx-arm64`, `win-64`). This lets a single URL work across platforms:
//!
//! ```text
//! gha://owner/repo/runs/<run-id>/my-package-{subdir}
//! ```
//!
//! Combined with a matrix build that uploads per-platform artifacts named
//! `my-package-linux-64`, `my-package-osx-arm64`, etc., rattler picks the
//! right artifact automatically without the caller needing to know the current
//! platform.
//!
//! ## Authentication
//!
//! GitHub tokens are resolved in this order:
//! 1. `GITHUB_TOKEN` environment variable
//! 2. `GH_TOKEN` environment variable
//! 3. `gh` CLI config file (`~/.config/gh/hosts.yml`)

use std::io::Cursor;
use std::path::{Path, PathBuf};

use rattler_conda_types::Platform;
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
        // path_segments() returns percent-encoded strings, so `{` → `%7B` and
        // `}` → `%7D`. Decode the braces so that `{subdir}` is recognised.
        let artifact_name = segments[3..]
            .join("/")
            .replace("%7B", "{")
            .replace("%7D", "}")
            .replace("%7b", "{")
            .replace("%7d", "}");

        // Substitute `{subdir}` with the current platform's conda subdirectory
        // (e.g. `linux-64`, `osx-arm64`, `win-64`). This allows a single URL
        // like `gha://owner/repo/runs/123/my-pkg-{subdir}` to resolve to the
        // correct per-platform artifact automatically.
        let artifact_name = if artifact_name.contains("{subdir}") {
            let subdir = Platform::current().as_str().to_string();
            artifact_name.replace("{subdir}", &subdir)
        } else {
            artifact_name
        };

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

    // 4. `gh auth token` — handles keychain-backed auth used by newer gh versions
    if let Some(token) = run_gh_auth_token() {
        return Ok(token);
    }

    Err(GhaError::NoToken)
}

/// Run `gh auth token` and return the token it prints, if the `gh` CLI is
/// available and the user is authenticated.
fn run_gh_auth_token() -> Option<String> {
    let output = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let token = String::from_utf8(output.stdout).ok()?;
    let token = token.trim().to_string();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
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

    // Check if we already have the package cached under any candidate name.
    // We probe the same candidate order used by find_artifact_id so that a
    // previously resolved fallback is found on the second call without hitting
    // the API again.
    let candidates = artifact_name_candidates(&gha_url.artifact_name);
    for candidate in &candidates {
        let dir = cache_dir
            .join("gha")
            .join(&gha_url.owner)
            .join(&gha_url.repo)
            .join(gha_url.run_id.to_string())
            .join(candidate);
        if let Some(cached) = find_conda_package(&dir) {
            tracing::debug!("using cached GHA artifact: {}", cached.display());
            return Ok(cached);
        }
    }

    // Find the artifact ID via the GitHub API. Returns the resolved name in
    // case a platform-suffix fallback was used.
    let (artifact_id, resolved_name) = find_artifact_id(client, &token, gha_url).await?;

    let artifact_cache_dir = cache_dir
        .join("gha")
        .join(&gha_url.owner)
        .join(&gha_url.repo)
        .join(gha_url.run_id.to_string())
        .join(&resolved_name);

    // Download the artifact zip
    let zip_bytes = download_artifact(client, &token, gha_url, artifact_id).await?;

    // Extract the conda package from the zip
    std::fs::create_dir_all(&artifact_cache_dir)?;
    extract_conda_from_zip(&zip_bytes, &artifact_cache_dir)?;

    find_conda_package(&artifact_cache_dir).ok_or_else(|| GhaError::NoCondaPackage(resolved_name))
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

/// Build the list of artifact name candidates to try, in priority order:
///
/// 1. The exact name as written.
/// 2. `<name>-<current-platform-subdir>` (e.g. `hello-rattler-linux-64`).
/// 3. `<name>-noarch`.
///
/// This lets users write a bare package name like `hello-rattler` in a
/// `gha://` URL and have rattler automatically find the right per-platform or
/// noarch artifact without them needing to know or hard-code the platform.
fn artifact_name_candidates(name: &str) -> Vec<String> {
    let subdir = Platform::current().as_str().to_string();
    vec![
        name.to_string(),
        format!("{name}-{subdir}"),
        format!("{name}-noarch"),
    ]
}

/// Find the artifact ID for a given artifact name in a workflow run.
///
/// Returns `(artifact_id, resolved_name)`. The resolved name may differ from
/// `gha_url.artifact_name` when a platform-suffix or noarch fallback was used.
async fn find_artifact_id(
    client: &ClientWithMiddleware,
    token: &str,
    gha_url: &GhaUrl,
) -> Result<(u64, String), GhaError> {
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

    // Build a map of artifact name → id for the O(candidates) lookup below.
    let artifact_map: std::collections::HashMap<&str, u64> = artifacts
        .iter()
        .filter_map(|a| {
            let name = a["name"].as_str()?;
            let id = a["id"].as_u64()?;
            Some((name, id))
        })
        .collect();

    // Try each candidate in priority order and return the first hit.
    for candidate in artifact_name_candidates(&gha_url.artifact_name) {
        if let Some(&id) = artifact_map.get(candidate.as_str()) {
            if candidate != gha_url.artifact_name {
                tracing::debug!(
                    "artifact '{}' not found; using fallback '{}'",
                    gha_url.artifact_name,
                    candidate,
                );
            }
            return Ok((id, candidate));
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
    fn test_artifact_name_candidates_exact() {
        // When the name already contains a platform suffix (or is otherwise
        // explicit), the exact name is always the first candidate.
        let candidates = artifact_name_candidates("hello-rattler-linux-64");
        assert_eq!(candidates[0], "hello-rattler-linux-64");
    }

    #[test]
    fn test_artifact_name_candidates_fallbacks() {
        let current = Platform::current().as_str().to_string();
        let candidates = artifact_name_candidates("hello-rattler");
        assert_eq!(candidates[0], "hello-rattler");
        assert_eq!(candidates[1], format!("hello-rattler-{current}"));
        assert_eq!(candidates[2], "hello-rattler-noarch");
    }

    #[test]
    fn test_parse_gha_url_subdir_substitution() {
        let current = Platform::current().as_str().to_string();
        let url =
            Url::parse("gha://wolfv/hello-rattler-gha/runs/99/hello-rattler-{subdir}").unwrap();
        let gha_url = GhaUrl::parse(&url).unwrap();
        assert_eq!(
            gha_url.artifact_name,
            format!("hello-rattler-{current}"),
            "`{{subdir}}` should be replaced with the current platform subdir"
        );
    }

    #[test]
    fn test_gh_config_path() {
        // Just make sure it doesn't panic
        let _ = gh_config_path();
    }
}
