//! Middleware to handle mirrors
use std::{
    collections::HashMap,
    sync::atomic::{self, AtomicUsize},
};

use http::{Extensions, StatusCode};
use itertools::Itertools;
use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next, Result};
use url::Url;

#[cfg(target_arch = "wasm32")]
use http::response::Builder;
#[cfg(target_arch = "wasm32")]
use reqwest::ResponseBuilderExt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Settings for the specific mirror (e.g. no zstd or bz2 support)
pub struct Mirror {
    /// The url of this mirror
    pub url: Url,
    /// Disable zstd support (for repodata.json.zst files)
    pub no_zstd: bool,
    /// Disable bz2 support (for repodata.json.bz2 files)
    pub no_bz2: bool,
    /// Disable jlap support (for repodata.jlap files)
    pub no_jlap: bool,
    /// Allowed number of failures before the mirror is considered dead
    pub max_failures: Option<usize>,
}

#[allow(dead_code)]
struct MirrorState {
    failures: AtomicUsize,
    mirror: Mirror,
}

impl MirrorState {
    pub fn add_failure(&self) {
        self.failures.fetch_add(1, atomic::Ordering::Relaxed);
    }
}

/// Middleware to handle mirrors
pub struct MirrorMiddleware {
    mirror_map: HashMap<Url, Vec<MirrorState>>,
    sorted_keys: Vec<(String, Url)>,
}

impl MirrorMiddleware {
    /// Create a new `MirrorMiddleware` from a map of mirrors
    pub fn from_map(mirror_map: HashMap<Url, Vec<Mirror>>) -> Self {
        let mirror_map: HashMap<Url, Vec<MirrorState>> = mirror_map
            .into_iter()
            .map(|(url, mirrors)| {
                let mirrors = mirrors
                    .into_iter()
                    .map(|mirror| MirrorState {
                        failures: AtomicUsize::new(0),
                        mirror,
                    })
                    .collect();
                (url, mirrors)
            })
            .collect();

        let sorted_keys = mirror_map
            .keys()
            .cloned()
            .sorted_by(|a, b| b.path().len().cmp(&a.path().len()))
            .map(|k| (k.to_string(), k.clone()))
            .collect::<Vec<(String, Url)>>();

        Self {
            mirror_map,
            sorted_keys,
        }
    }

    /// Get sorted keys. The keys are sorted by length of the path,
    /// so the longest path comes first.
    pub fn keys(&self) -> &[(String, Url)] {
        &self.sorted_keys
    }
}

fn select_mirror(mirrors: &[MirrorState]) -> Option<&MirrorState> {
    let mut min_failures = usize::MAX;
    let mut min_failures_index = usize::MAX;

    for (i, mirror) in mirrors.iter().enumerate() {
        let failures = mirror.failures.load(atomic::Ordering::Relaxed);
        if failures < min_failures && mirror.mirror.max_failures.is_none_or(|max| failures < max) {
            min_failures = failures;
            min_failures_index = i;
        }
    }
    if min_failures_index == usize::MAX {
        return None;
    }
    Some(&mirrors[min_failures_index])
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Middleware for MirrorMiddleware {
    async fn handle(
        &self,
        mut req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        let url_str = req.url().to_string();

        for (key, url) in self.keys() {
            if let Some(url_rest) = url_str.strip_prefix(key) {
                let url_rest = url_rest.trim_start_matches('/');
                // replace the key with the mirror
                let mirrors = self.mirror_map.get(url).unwrap();
                let selected_mirror = select_mirror(mirrors);

                let Some(selected_mirror) = selected_mirror else {
                    return Ok(create_404_response(req.url(), "All mirrors are dead"));
                };

                let mirror = &selected_mirror.mirror;
                let selected_url = mirror.url.join(url_rest).unwrap();

                // Short-circuit if the mirror does not support the file type
                if url_rest.ends_with(".json.zst") && mirror.no_zstd {
                    return Ok(create_404_response(
                        &selected_url,
                        "Mirror does not support zstd",
                    ));
                }
                if url_rest.ends_with(".json.bz2") && mirror.no_bz2 {
                    return Ok(create_404_response(
                        &selected_url,
                        "Mirror does not support bz2",
                    ));
                }
                if url_rest.ends_with(".jlap") && mirror.no_jlap {
                    return Ok(create_404_response(
                        &selected_url,
                        "Mirror does not support jlap",
                    ));
                }

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

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn create_404_response(url: &Url, body: &str) -> Response {
    use reqwest::ResponseBuilderExt;
    Response::from(
        http::response::Builder::new()
            .status(StatusCode::NOT_FOUND)
            .url(url.clone())
            .body(body.to_string())
            .unwrap(),
    )
}

/// Creates a 404 Not Found response for WASM targets.
///
/// # Arguments
/// * `url` - The URL that was not found
/// * `body` - The error message to include in the response
///
/// # Returns
/// A [`reqwest::Response`] with a 404 status code and the given body
#[cfg(target_arch = "wasm32")]
pub(crate) fn create_404_response(url: &Url, body: &str) -> Response {
    Response::from(
        Builder::new()
            .status(StatusCode::NOT_FOUND)
            .url(url.clone())
            .header("Content-Type", "text/plain")
            .header("Content-Length", body.len().to_string())
            .body(body.to_string())
            .unwrap(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::*;

    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_test_configure!(run_in_browser);

    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen_test]
    async fn test_wasm_404_response() {
        let url = Url::parse("http://example.com").unwrap();
        let body = "Mirror does not support zstd";
        let response = create_404_response(&url, body);

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers().get("Content-Type").unwrap(),
            "text/plain"
        );
        assert_eq!(
            response.headers().get("Content-Length").unwrap(),
            body.len().to_string()
        );

        let text = response.text().await.unwrap();
        assert_eq!(text, body);
    }
}
