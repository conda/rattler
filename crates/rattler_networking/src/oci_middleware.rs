//! Middleware to handle `oci://` URLs to pull artifacts from an OCI registry
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use http::header::{ACCEPT, AUTHORIZATION};
use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next, Result};
use serde::Deserialize;
use task_local_extensions::Extensions;
use url::Url;

use crate::mirror_middleware::create_404_response;

/// Middleware to handle `oci://` URLs
#[derive(Default, Debug, Clone)]
pub struct OciMiddleware {
    token_cache: Arc<Mutex<HashMap<Url, String>>>,
}

#[allow(dead_code)]
enum OciAction {
    Pull,
    Push,
    PushPull,
}

#[derive(Clone, Debug, Deserialize)]
struct OCIToken {
    token: String,
}

impl ToString for OciAction {
    fn to_string(&self) -> String {
        match self {
            OciAction::Pull => "pull".to_string(),
            OciAction::Push => "push".to_string(),
            OciAction::PushPull => "push,pull".to_string(),
        }
    }
}
// [oci://ghcr.io/channel-mirrors/conda-forge]/[osx-arm64/xtensor]
async fn get_token(url: &Url, action: OciAction) -> Result<String> {
    let token_url: String = format!(
        "https://{}/token?scope=repository:{}:{}",
        url.host_str().unwrap(),
        &url.path()[1..],
        action.to_string()
    );

    tracing::info!("Requesting token from {}", token_url);

    let token = reqwest::get(token_url)
        .await
        .map_err(reqwest_middleware::Error::Reqwest)?
        .json::<OCIToken>()
        .await?
        .token;

    Ok(token)
}

fn oci_url_with_hash(url: &Url, hash: &str) -> Url {
    format!(
        "https://{}/v2{}/blobs/sha256:{}",
        url.host_str().unwrap(),
        url.path(),
        hash
    )
    .parse()
    .unwrap()
}

#[derive(Debug)]
struct OciTagMediaType {
    url: Url,
    tag: String,
    media_type: String,
}

#[allow(dead_code)]
fn reverse_version_build_tag(tag: &str) -> String {
    tag.replace("__p__", "+")
        .replace("__e__", "!")
        .replace("__eq__", "=")
}

fn version_build_tag(tag: &str) -> String {
    tag.replace('+', "__p__")
        .replace('!', "__e__")
        .replace('=', "__eq__")
}

/// We reimplement some logic from rattler here because we don't want to introduce cyclic dependencies
fn package_to_tag(url: &Url) -> OciTagMediaType {
    // get filename (last segment of path)
    let filename = url.path_segments().unwrap().last().unwrap();

    let mut res = OciTagMediaType {
        url: url.clone(),
        tag: "latest".to_string(),
        media_type: "".to_string(),
    };

    let mut computed_filename = filename.to_string();

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
        }
    }

    if computed_filename.starts_with('_') {
        computed_filename = format!("zzz{computed_filename}");
    }

    res.url = url.join(&computed_filename).unwrap();
    res
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
    media_type: String,
    layers: Vec<Layer>,
    config: Layer,
    annotations: Option<HashMap<String, String>>,
}

#[async_trait::async_trait]
impl Middleware for OciMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        // if the URL is not an OCI URL, we don't need to do anything
        if req.url().scheme() != "oci" {
            return next.run(req, extensions).await;
        }

        let oci_info = package_to_tag(req.url());
        let url = &oci_info.url;
        let token = self.token_cache.lock().unwrap().get(url).cloned();

        let token = if let Some(token) = token {
            token
        } else {
            let token = get_token(url, OciAction::Pull).await?;
            self.token_cache
                .lock()
                .unwrap()
                .insert(url.clone(), token.clone());
            token
        };

        let mut req = req;
        req.headers_mut()
            .insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());

        // if we know the hash, we can pull the artifact directly
        // if we don't, we need to pull the manifest and then pull the artifact
        if let Some(expected_sha_hash) = req
            .headers()
            .get("X-ExpectedSha256")
            .map(|s| s.to_str().unwrap().to_string())
        {
            *req.url_mut() = oci_url_with_hash(url, &expected_sha_hash);
            next.run(req, extensions).await
        } else {
            // get the tag from the URL
            // retrieve the manifest
            let manifest_url = format!(
                "https://{}/v2{}/manifests/{}",
                url.host_str().unwrap(),
                url.path(),
                &oci_info.tag
            );

            let manifest = reqwest::Client::new()
                .get(&manifest_url)
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .header(ACCEPT, "application/vnd.oci.image.manifest.v1+json")
                .send()
                .await
                .map_err(reqwest_middleware::Error::Reqwest)?;

            let manifest: Manifest = manifest.json().await?;

            let layer = if let Some(layer) = manifest
                .layers
                .iter()
                .find(|l| l.media_type == oci_info.media_type)
            {
                layer
            } else {
                return Ok(create_404_response(
                    url,
                    "No layer available for media type",
                ));
            };

            let layer_url = format!(
                "https://{}/v2{}/blobs/{}",
                url.host_str().unwrap(),
                url.path(),
                layer.digest
            );
            *req.url_mut() = layer_url.parse().unwrap();
            next.run(req, extensions).await
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use crate::OciMiddleware;

    // test pulling an image from OCI registry
    #[tokio::test]
    async fn test_oci_middleware() {
        let middleware = OciMiddleware::default();

        let client = reqwest::Client::new();
        let client_with_middleware = reqwest_middleware::ClientBuilder::new(client)
            .with(middleware)
            .build();

        let response = client_with_middleware
            .get("oci://ghcr.io/channel-mirrors/conda-forge/osx-arm64/xtensor-0.25.0-h2ffa867_0.conda")
            .header(
                "X-ExpectedSha256",
                "8485a64911c7011c0270b8266ab2bffa1da41c59ac4f0a48000c31d4f4a966dd",
            )
            .send()
            .await
            .unwrap();

        // write out to tempfile
        let mut file = std::fs::File::create("./test.tar.bz2").unwrap();
        assert_eq!(response.status(), 200);

        file.write_all(&response.bytes().await.unwrap()).unwrap();
    }

    // test pulling an image from OCI registry
    #[tokio::test]
    async fn test_oci_middleware_repodata() {
        let middleware = OciMiddleware::default();

        let client = reqwest::Client::new();
        let client_with_middleware = reqwest_middleware::ClientBuilder::new(client)
            .with(middleware)
            .build();

        let response = client_with_middleware
            .get("oci://ghcr.io/channel-mirrors/conda-forge/osx-arm64/repodata.json")
            .send()
            .await
            .unwrap();

        // write out to tempfile
        let mut file = std::fs::File::create("./test.json").unwrap();
        assert_eq!(response.status(), 200);

        file.write_all(&response.bytes().await.unwrap()).unwrap();
    }
}
