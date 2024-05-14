//! Middleware to handle mirrors
use std::{
    collections::HashMap,
    sync::atomic::{self, AtomicUsize},
};

use http::{Extensions, StatusCode};
use itertools::Itertools;
use reqwest::{Request, Response, ResponseBuilderExt};
use reqwest_middleware::{Middleware, Next, Result};
use url::Url;

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
        if failures < min_failures
            && mirror
                .mirror
                .max_failures
                .map_or(true, |max| failures < max)
        {
            min_failures = failures;
            min_failures_index = i;
        }
    }
    if min_failures_index == usize::MAX {
        return None;
    }
    Some(&mirrors[min_failures_index])
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
    use std::{future::IntoFuture, net::SocketAddr};

    use axum::{extract::State, http::StatusCode, routing::get, Router};
    use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
    use url::Url;

    use crate::MirrorMiddleware;

    use super::Mirror;

    async fn count(State(name): State<String>) -> String {
        format!("Hi from counter: {name}")
    }

    async fn broken_return() -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }

    async fn test_server(name: &str, broken: bool) -> Url {
        let state = String::from(name);

        // Construct a router that returns data from the static dir but fails the first try.
        let router = if broken {
            Router::new().route("/count", get(broken_return))
        } else {
            Router::new().route("/count", get(count)).with_state(state)
        };

        let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
        let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let service = router.into_make_service();
        tokio::spawn(axum::serve(listener, service).into_future());
        format!("http://{}:{}", addr.ip(), addr.port())
            .parse()
            .unwrap()
    }

    #[tokio::test]
    async fn test_mirror_middleware() {
        let addr_1 = test_server("server 1", false).await;
        let addr_2 = test_server("server 2", false).await;

        let mut mirror_map = std::collections::HashMap::new();

        mirror_map.insert(
            "http://bla.com".parse().unwrap(),
            vec![mirror_setting(addr_1), mirror_setting(addr_2)],
        );

        let middleware = crate::MirrorMiddleware::from_map(mirror_map);
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
            .with(middleware)
            .build();

        let res = client.get("http://bla.com/count").send().await.unwrap();
        assert!(res.status().is_success());
        let res = res.text().await.unwrap();
        println!("{res}");
        // should always take the first element from the list
        assert!(res == "Hi from counter: server 1");
    }

    fn mirror_setting(url: Url) -> Mirror {
        Mirror {
            url,
            no_zstd: false,
            no_bz2: false,
            no_jlap: false,
            max_failures: Some(3),
        }
    }

    #[tokio::test]
    async fn test_mirror_middleware_broken() {
        let addr_1 = test_server("server 1", true).await;
        let addr_2 = test_server("server 2", false).await;

        let mut mirror_map = std::collections::HashMap::new();

        mirror_map.insert(
            "http://bla.com".parse().unwrap(),
            vec![mirror_setting(addr_1), mirror_setting(addr_2)],
        );

        let middleware = MirrorMiddleware::from_map(mirror_map.clone());
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
            .with(middleware)
            .build();

        let res = client.get("http://bla.com/count").send().await.unwrap();
        assert!(res.status().is_server_error());
        // only the second server should be used
        let res = client.get("http://bla.com/count").send().await.unwrap();
        assert!(res.status().is_success());
        assert!(res.text().await.unwrap() == "Hi from counter: server 2");

        // add retry handler
        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);

        let middleware = MirrorMiddleware::from_map(mirror_map);
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
            // retry middleware has to come before the mirror middleware
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .with(middleware)
            .build();

        let res = client.get("http://bla.com/count").send().await.unwrap();
        assert!(res.status().is_success());
        assert!(res.text().await.unwrap() == "Hi from counter: server 2");
    }

    #[test]
    fn test_mirror_sort() {
        let keys: Vec<Url> = vec![
            "http://bla.com/abc/def".parse().unwrap(),
            "http://bla.com/abc".parse().unwrap(),
            "http://bla.com/abc/def/ghi".parse().unwrap(),
        ];

        let mirror_middleware =
            MirrorMiddleware::from_map(keys.into_iter().map(|k| (k.clone(), vec![])).collect());

        let mut len = mirror_middleware.keys()[0].0.len();
        for path in mirror_middleware.keys().iter() {
            assert!(path.0.len() <= len);
            len = path.0.len();
        }
    }
}
