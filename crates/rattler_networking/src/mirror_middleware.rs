//! Middleware to handle mirrors
use std::{
    collections::HashMap,
    sync::atomic::{self, AtomicUsize},
};

use http::StatusCode;
use reqwest::{Request, Response, ResponseBuilderExt};
use reqwest_middleware::{Middleware, Next, Result};
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

pub(crate) fn create_404_response(url: &Url, body: &str) -> Response {
    Response::from(
        http::response::Builder::new()
            .status(StatusCode::NOT_FOUND)
            .url(url.clone())
            .body(body.to_string())
            .unwrap(),
    )
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
}
