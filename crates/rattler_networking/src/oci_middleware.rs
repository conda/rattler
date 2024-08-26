//! Middleware to handle `oci://` URLs to pull artifacts from an OCI registry
use std::{
    collections::HashMap,
    fmt::{Display, Formatter},
};

use http::{
    header::{ACCEPT, AUTHORIZATION},
    Extensions,
};
use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next};
use serde::Deserialize;
use url::{ParseError, Url};

use crate::mirror_middleware::create_404_response;

#[derive(thiserror::Error, Debug)]
enum OciMiddlewareError {
    #[error("Reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("URL parse error: {0}")]
    ParseError(#[from] ParseError),

    #[error("Layer not found")]
    LayerNotFound,
}

/// Middleware to handle `oci://` URLs
#[derive(Default, Debug, Clone)]
pub struct OciMiddleware;

/// The action to perform on the OCI registry
pub enum OciAction {
    /// Pull an artifact
    Pull,
    /// Push an artifact
    Push,
    /// Push and/or pull an artifact
    PushPull,
}

#[derive(Clone, Debug, Deserialize)]
struct OCIToken {
    token: String,
}

impl Display for OciAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            OciAction::Pull => write!(f, "pull"),
            OciAction::Push => write!(f, "push"),
            OciAction::PushPull => write!(f, "push,pull"),
        }
    }
}

// [oci://ghcr.io/channel-mirrors/conda-forge]/[osx-arm64/xtensor]
async fn get_token(url: &OCIUrl, action: OciAction) -> Result<String, OciMiddlewareError> {
    let token_url = url.token_url(action)?;

    tracing::trace!("OCI Mirror: requesting token from {}", token_url);

    let token = reqwest::get(token_url)
        .await?
        .json::<OCIToken>()
        .await?
        .token;

    Ok(token)
}

#[derive(Debug)]
struct OCIUrl {
    url: Url,
    host: String,
    path: String,
    tag: String,
    media_type: String,
}

/// OCI registry tags are not allowed to contain `+`, `!`, or `=`, so we need to
/// replace them with something else (reverse of `version_build_tag`)
#[allow(dead_code)]
fn reverse_version_build_tag(tag: &str) -> String {
    tag.replace("__p__", "+")
        .replace("__e__", "!")
        .replace("__eq__", "=")
}

/// OCI registry tags are not allowed to contain `+`, `!`, or `=`, so we need to
/// replace them with something else
fn version_build_tag(tag: &str) -> String {
    tag.replace('+', "__p__")
        .replace('!', "__e__")
        .replace('=', "__eq__")
}

impl OCIUrl {
    pub fn manifest_url(&self) -> Result<Url, ParseError> {
        format!(
            "https://{}/v2/{}/manifests/{}",
            self.host, self.path, self.tag
        )
        .parse()
    }

    pub fn token_url(&self, action: OciAction) -> Result<Url, ParseError> {
        format!(
            "https://{}/token?scope=repository:{}:{}",
            self.host, self.path, action
        )
        .parse()
    }

    pub fn blob_url(&self, sha256: &str) -> Result<Url, ParseError> {
        format!("https://{}/v2/{}/blobs/{}", self.host, self.path, sha256).parse()
    }

    pub fn new(url: &Url) -> Result<Self, ParseError> {
        // get filename (last segment of path)
        let filename = url.path_segments().unwrap().last().unwrap();

        let mut res = OCIUrl {
            url: url.clone(),
            tag: "latest".to_string(),
            media_type: "".to_string(),
            host: url.host_str().unwrap_or("").to_string(),
            path: url.path().trim_start_matches('/').to_string(),
        };

        let mut computed_filename = filename.to_string();

        // We reimplement some archive name splitting logic from rattler here
        // because we don't want to introduce cyclic dependencies
        if let Some(archive_name) = filename.strip_suffix(".conda") {
            let parts = archive_name.rsplitn(3, '-').collect::<Vec<&str>>();
            computed_filename = parts[2].to_string();
            res.tag = version_build_tag(&format!("{}-{}", parts[1], parts[0]));
            res.media_type = "application/vnd.conda.package.v2".to_string();
        } else if let Some(archive_name) = filename.strip_suffix(".tar.bz2") {
            let parts = archive_name.rsplitn(3, '-').collect::<Vec<&str>>();
            computed_filename = parts[2].to_string();
            res.tag = version_build_tag(&format!("{}-{}", parts[1], parts[0]));
            res.media_type = "application/vnd.conda.package.v1".to_string();
        } else if filename.starts_with("repodata.json") {
            computed_filename = "repodata.json".to_string();
            if filename == "repodata.json" {
                res.media_type = "application/vnd.conda.repodata.v1+json".to_string();
            } else if filename.ends_with(".gz") {
                res.media_type = "application/vnd.conda.repodata.v1+json+gzip".to_string();
            } else if filename.ends_with(".bz2") {
                res.media_type = "application/vnd.conda.repodata.v1+json+bz2".to_string();
            } else if filename.ends_with(".zst") {
                res.media_type = "application/vnd.conda.repodata.v1+json+zst".to_string();
            } else if filename.ends_with(".jlap") {
                res.media_type = "application/vnd.conda.jlap.v1".to_string();
            }
        }

        // OCI image names cannot start with `_`, so we prefix it with `zzz`
        if computed_filename.starts_with('_') {
            computed_filename = format!("zzz{computed_filename}");
        }

        res.url = url.join(&computed_filename).unwrap();
        res.path = res.url.path().trim_start_matches('/').to_string();
        Ok(res)
    }

    pub async fn get_blob_url(req: &mut Request) -> Result<(), OciMiddlewareError> {
        let oci_url = OCIUrl::new(req.url())?;
        let token = get_token(&oci_url, OciAction::Pull).await?;

        req.headers_mut().insert(
            AUTHORIZATION,
            format!("Bearer {token}")
                .parse()
                .expect("Could not parse token header"),
        );

        // if we know the hash, we can pull the artifact directly
        // if we don't, we need to pull the manifest and then pull the artifact
        if let Some(expected_sha_hash) = req
            .headers()
            .get("X-Expected-Sha256")
            .and_then(|s| s.to_str().ok())
        {
            *req.url_mut() = oci_url.blob_url(&format!("sha256:{expected_sha_hash}"))?;
        } else {
            // get the tag from the URL retrieve the manifest
            let manifest_url = oci_url.manifest_url()?; // TODO: handle error

            let manifest = reqwest::Client::new()
                .get(manifest_url)
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .header(ACCEPT, "application/vnd.oci.image.manifest.v1+json")
                .send()
                .await?;

            let manifest: Manifest = manifest.json().await?;

            let layer = if let Some(layer) = manifest
                .layers
                .iter()
                .find(|l| l.media_type == oci_url.media_type)
            {
                layer
            } else {
                return Err(OciMiddlewareError::LayerNotFound);
            };

            *req.url_mut() = oci_url.blob_url(&layer.digest)?;
        }

        Ok(())
    }
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct Layer {
    digest: String,
    #[serde(rename = "mediaType")]
    media_type: String,
    size: u64,
    annotations: Option<HashMap<String, String>>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: u64,
    layers: Vec<Layer>,
    config: Layer,
    annotations: Option<HashMap<String, String>>,
}

#[async_trait::async_trait]
impl Middleware for OciMiddleware {
    async fn handle(
        &self,
        mut req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        // if the URL is not an OCI URL, we don't need to do anything
        if req.url().scheme() != "oci" {
            return next.run(req, extensions).await;
        }

        // return 404 for the moment as these are not supported
        if req.url().path().ends_with(".jlap") || req.url().path().ends_with(".json.bz2") {
            return Ok(create_404_response(
                req.url(),
                "Mirror does not support this file type",
            ));
        }

        let res = OCIUrl::get_blob_url(&mut req).await;

        match res {
            Ok(_) => next.run(req, extensions).await,
            Err(e) => match e {
                OciMiddlewareError::LayerNotFound => {
                    return Ok(create_404_response(
                        req.url(),
                        "No layer available for media type",
                    ));
                }
                _ => {
                    return Err(reqwest_middleware::Error::Middleware(e.into()));
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use crate::OciMiddleware;

    // test pulling an image from OCI registry
    #[cfg(any(feature = "rustls-tls", feature = "native-tls"))]
    #[tokio::test]
    async fn test_oci_middleware() {
        let middleware = OciMiddleware;

        let client = reqwest::Client::new();
        let client_with_middleware = reqwest_middleware::ClientBuilder::new(client)
            .with(middleware)
            .build();

        let response = client_with_middleware
            .get("oci://ghcr.io/channel-mirrors/conda-forge/osx-arm64/xtensor-0.25.0-h2ffa867_0.conda")
            .header(
                "X-Expected-Sha256",
                "8485a64911c7011c0270b8266ab2bffa1da41c59ac4f0a48000c31d4f4a966dd",
            )
            .send()
            .await
            .unwrap();

        // write out to tempfile
        assert_eq!(response.status(), 200);
        // check that the bytes are the same
        let mut hasher = Sha256::new();
        std::io::copy(&mut response.bytes().await.unwrap().as_ref(), &mut hasher).unwrap();
        let hash = hasher.finalize();
        assert_eq!(
            format!("{hash:x}"),
            "8485a64911c7011c0270b8266ab2bffa1da41c59ac4f0a48000c31d4f4a966dd"
        );
    }

    // test pulling an image from OCI registry
    #[cfg(any(feature = "rustls-tls", feature = "native-tls"))]
    #[tokio::test]
    async fn test_oci_middleware_repodata() {
        let middleware = OciMiddleware;

        let client = reqwest::Client::new();
        let client_with_middleware = reqwest_middleware::ClientBuilder::new(client)
            .with(middleware)
            .build();

        let response = client_with_middleware
            .head("oci://ghcr.io/channel-mirrors/conda-forge/osx-arm64/repodata.json")
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
    }
}
