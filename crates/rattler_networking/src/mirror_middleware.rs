//! Middleware to handle mirrors
use std::{
    collections::HashMap,
    sync::{
        atomic::{self, AtomicUsize},
        Arc, Mutex,
    },
};

use http::StatusCode;
use reqwest::{
    header::{ACCEPT, AUTHORIZATION},
    Request, Response, ResponseBuilderExt,
};
use reqwest_middleware::{Middleware, Next, Result};
use serde::Deserialize;
use task_local_extensions::Extensions;
use url::Url;

#[allow(dead_code)]
/// Settings for the specific mirror (e.g. no zstd or bz2 support)
struct MirrorSettings {
    no_zstd: bool,
    no_bz2: bool,
    no_gz: bool,
    max_failures: Option<usize>,
}

#[allow(dead_code)]
struct MirrorState {
    url: Url,

    failures: AtomicUsize,

    settings: MirrorSettings,
}

impl MirrorState {
    pub fn add_failure(&self) {
        self.failures.fetch_add(1, atomic::Ordering::Relaxed);
    }
}

/// Middleware to handle mirrors
pub struct MirrorMiddleware {
    mirror_map: HashMap<String, Vec<MirrorState>>,
}

impl MirrorMiddleware {
    /// Create a new `MirrorMiddleware` from a map of mirrors
    pub fn from_map(map: HashMap<String, Vec<String>>) -> Self {
        let mirror_map = map
            .into_iter()
            .map(|(k, v)| {
                let v = v
                    .into_iter()
                    .map(|url| {
                        let url = if url.ends_with('/') {
                            url
                        } else {
                            format!("{url}/")
                        };
                        MirrorState {
                            url: Url::parse(&url).unwrap(),
                            failures: AtomicUsize::new(0),
                            settings: MirrorSettings {
                                no_zstd: false,
                                no_bz2: false,
                                no_gz: false,
                                max_failures: Some(3),
                            },
                        }
                    })
                    .collect();
                (k, v)
            })
            .collect();

        Self { mirror_map }
    }
}

fn select_mirror(mirrors: &[MirrorState]) -> &MirrorState {
    let mut min_failures = usize::MAX;
    let mut min_failures_index = 0;

    for (i, mirror) in mirrors.iter().enumerate() {
        let failures = mirror.failures.load(atomic::Ordering::Relaxed);
        if failures < min_failures {
            min_failures = failures;
            min_failures_index = i;
        }
    }

    &mirrors[min_failures_index]
}

#[async_trait::async_trait]
impl Middleware for MirrorMiddleware {
    async fn handle(
        &self,
        mut req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        let url_str = req.url().to_string();

        for (key, mirrors) in self.mirror_map.iter() {
            if let Some(url_rest) = url_str.strip_prefix(key) {
                let url_rest = url_rest.trim_start_matches('/');
                // replace the key with the mirror
                let selected_mirror = select_mirror(mirrors);
                let selected_url = selected_mirror.url.join(url_rest).unwrap();
                *req.url_mut() = selected_url;
                let res = next.run(req, extensions).await;

                // record a failure if the request failed so we can avoid the mirror in the future
                match res.as_ref() {
                    Ok(res) if res.status().is_server_error() => selected_mirror.add_failure(),
                    Err(_) => selected_mirror.add_failure(),
                    _ => {}
                }

                return res;
            }
        }

        // if we don't have a mirror, we don't need to do anything
        next.run(req, extensions).await
    }
}

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

impl ToString for OciAction {
    fn to_string(&self) -> String {
        match self {
            OciAction::Pull => "pull".to_string(),
            OciAction::Push => "push".to_string(),
            OciAction::PushPull => "push,pull".to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct OCIToken {
    token: String,
}

pub(crate) fn create_404_response(url: &Url, body: &str) -> Response {
    Response::from(
        http::response::Builder::new()
            .status(StatusCode::NOT_FOUND)
            .url(url.clone())
            .body(body.to_string())
            .unwrap(),
    )
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
mod test {
    use std::io::Write;

    use super::*;

    // #[tokio::test]
    // async fn test_mirror_middleware() {
    //     let mut mirror_map = HashMap::new();
    //     mirror_map.insert(
    //         "conda.anaconda.org".to_string(),
    //         vec![
    //             "https://conda.anaconda.org/conda-forge".to_string(),
    //             "https://conda.anaconda.org/conda-forge".to_string(),
    //         ],
    //     );

    //     let middleware = MirrorMiddleware::from_map(mirror_map);

    //     let client = reqwest::Client::new();
    //     let mut extensions = Extensions::new();

    //     let response = middleware
    //         .handle(
    //             client.get("https://conda.anaconda.org/conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.tar.bz2"),
    //             &mut extensions,
    //             |req, _| async { Ok(req.send().await.unwrap()) },
    //         )
    //         .await
    //         .unwrap();

    //     assert_eq!(response.status(), 200);
    // }

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
