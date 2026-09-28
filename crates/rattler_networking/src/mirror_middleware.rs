//! Middleware to handle mirrors
use std::{
    collections::HashMap,
    sync::atomic::{self, AtomicUsize},
};

use http::Extensions;
use itertools::Itertools;
use reqwest::{Request, Response};
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
    /// Create a new `MirrorMiddleware` from a map of mirrors.
    ///
    /// URLs are normalized to directory prefixes ending in `/`; bare channel
    /// paths without the slash are not mirrored. Equivalent source URLs combine
    /// their mirror lists in source-URL order.
    pub fn from_map(mirror_map: HashMap<Url, Vec<Mirror>>) -> Self {
        fn with_trailing_slash(url: &Url) -> Url {
            if url.path().ends_with('/') {
                url.clone()
            } else {
                let mut url = url.clone();
                url.set_path(&format!("{}/", url.path()));
                url
            }
        }

        let mut normalized_map: HashMap<Url, Vec<MirrorState>> =
            HashMap::with_capacity(mirror_map.len());
        for (url, mirrors) in mirror_map
            .into_iter()
            .sorted_by(|(a, _), (b, _)| a.as_str().cmp(b.as_str()))
        {
            normalized_map
                .entry(with_trailing_slash(&url))
                .or_default()
                .extend(mirrors.into_iter().map(|mut mirror| {
                    mirror.url = with_trailing_slash(&mirror.url);
                    MirrorState {
                        failures: AtomicUsize::new(0),
                        mirror,
                    }
                }));
        }

        let sorted_keys = normalized_map
            .keys()
            .cloned()
            .sorted_by(|a, b| b.path().len().cmp(&a.path().len()))
            .map(|k| (k.to_string(), k.clone()))
            .collect::<Vec<(String, Url)>>();

        Self {
            mirror_map: normalized_map,
            sorted_keys,
        }
    }

    /// Get sorted keys. The keys are sorted by length of the path,
    /// so the longest path comes first.
    pub fn keys(&self) -> &[(String, Url)] {
        &self.sorted_keys
    }

    /// Create a new `MirrorMiddleware` from the `mirrors` map of the shared
    /// rattler configuration (see [`rattler_config`]).
    ///
    /// Accepts a [`rattler_config::config::CommonConfig`]; a
    /// `&ConfigBase<T>` of any extension coerces into it.
    #[cfg(feature = "rattler_config")]
    pub fn from_config(config: &rattler_config::config::CommonConfig) -> Self {
        Self::from_map(
            config
                .mirrors
                .iter()
                .map(|(url, mirrors)| {
                    (
                        url.clone(),
                        mirrors
                            .iter()
                            .map(|mirror| Mirror {
                                url: mirror.clone(),
                                no_zstd: false,
                                no_bz2: false,
                                max_failures: None,
                            })
                            .collect(),
                    )
                })
                .collect(),
        )
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
                let selected_url = {
                    let mut u = mirror.url.clone();
                    let base_path = u.path().trim_end_matches('/');
                    if url_rest.is_empty() {
                        u.set_path(&format!("{base_path}/"));
                    } else {
                        u.set_path(&format!("{base_path}/{url_rest}"));
                    }
                    u
                };

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
            .status(http::StatusCode::NOT_FOUND)
            .url(url.clone())
            .body(body.to_string())
            .unwrap(),
    )
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn create_404_response(_url: &Url, _body: &str) -> Response {
    todo!("This is not implemented in reqwest, we need to contribute that.")
}

#[cfg(test)]
mod test {
    use std::{future::IntoFuture, net::SocketAddr};

    use axum::{Router, extract::State, http::StatusCode, routing::get};
    use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
    use url::Url;

    use crate::MirrorMiddleware;

    use super::Mirror;

    async fn count(State(name): State<String>) -> String {
        format!("Hi from counter: {name}")
    }

    async fn broken_return() -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }

    /// Echoes the server identity and request path to detect misrouting.
    async fn echo_path_prefixed(prefix: &'static str, req: axum::extract::Request) -> String {
        format!("HIT {prefix} at path: {}", req.uri().path())
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

    #[tokio::test]
    async fn test_mirror_middleware_path_rewrite() {
        // Start a server that serves at /channel/count
        let state = String::from("mirror server");
        let router = Router::new()
            .route("/channel/count", get(count))
            .with_state(state);

        let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
        let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, router.into_make_service()).into_future());

        let mirror_url: Url = format!("http://{}:{}/channel", addr.ip(), addr.port())
            .parse()
            .unwrap();

        let mut mirror_map = std::collections::HashMap::new();

        // Upstream key includes a path segment (e.g. conda-forge)
        // Mirror URL also has a path segment (e.g. channel)
        // The mirror path must fully replace the upstream path.
        mirror_map.insert(
            "https://prefix.dev/conda-forge".parse().unwrap(),
            vec![mirror_setting(mirror_url)],
        );

        let middleware = MirrorMiddleware::from_map(mirror_map);
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
            .with(middleware)
            .build();

        // Request to upstream: https://prefix.dev/conda-forge/count
        // Should be rewritten to: http://127.0.0.1:PORT/channel/count
        let res = client
            .get("https://prefix.dev/conda-forge/count")
            .send()
            .await
            .unwrap();
        assert!(res.status().is_success(), "status: {}", res.status());
        let body = res.text().await.unwrap();
        assert_eq!(body, "Hi from counter: mirror server");
    }

    #[tokio::test]
    async fn test_mirror_middleware_does_not_cross_channel_boundary() {
        // Echo requests received by the conda-forge mirror.
        let mirror_router = Router::new()
            .fallback(|req: axum::extract::Request| echo_path_prefixed("conda-forge mirror", req));
        let mirror_addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
        let mirror_listener = tokio::net::TcpListener::bind(&mirror_addr).await.unwrap();
        let mirror_addr = mirror_listener.local_addr().unwrap();
        tokio::spawn(axum::serve(mirror_listener, mirror_router.into_make_service()).into_future());
        let mirror_url: Url = format!("http://{}:{}", mirror_addr.ip(), mirror_addr.port())
            .parse()
            .unwrap();

        // Unmatched channels should reach this upstream server unchanged.
        let upstream_router = Router::new()
            .fallback(|req: axum::extract::Request| echo_path_prefixed("real upstream", req));
        let upstream_addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
        let upstream_listener = tokio::net::TcpListener::bind(&upstream_addr).await.unwrap();
        let upstream_addr = upstream_listener.local_addr().unwrap();
        tokio::spawn(
            axum::serve(upstream_listener, upstream_router.into_make_service()).into_future(),
        );

        for suffix in ["", "/"] {
            let mirror_map = std::collections::HashMap::from([(
                format!("http://{upstream_addr}/conda-forge{suffix}")
                    .parse()
                    .unwrap(),
                vec![mirror_setting(mirror_url.clone())],
            )]);
            let middleware = MirrorMiddleware::from_map(mirror_map);
            let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
                .with(middleware)
                .build();

            for (path, expected) in [
                (
                    "conda-forge2/count",
                    "HIT real upstream at path: /conda-forge2/count",
                ),
                (
                    "conda-forge/count",
                    "HIT conda-forge mirror at path: /count",
                ),
                ("conda-forge/", "HIT conda-forge mirror at path: /"),
                // A bare channel URL is not inside the normalized directory prefix.
                ("conda-forge", "HIT real upstream at path: /conda-forge"),
            ] {
                let res = client
                    .get(format!("http://{upstream_addr}/{path}"))
                    .send()
                    .await
                    .unwrap();
                assert!(res.status().is_success(), "status: {}", res.status());
                assert_eq!(
                    res.text().await.unwrap(),
                    expected,
                    "key suffix {suffix:?}, path {path}"
                );
            }
        }
    }

    #[test]
    fn from_map_appends_trailing_slashes() {
        for source_suffix in ["", "/"] {
            for mirror_suffix in ["", "/"] {
                let middleware = MirrorMiddleware::from_map(std::collections::HashMap::from([(
                    format!("https://upstream.example.com/channel{source_suffix}")
                        .parse()
                        .unwrap(),
                    vec![mirror_setting(
                        format!("https://mirror.example.com/channel{mirror_suffix}")
                            .parse()
                            .unwrap(),
                    )],
                )]));
                let source: Url = "https://upstream.example.com/channel/".parse().unwrap();
                assert_eq!(middleware.keys(), [(source.to_string(), source.clone())]);
                assert_eq!(
                    middleware.mirror_map[&source][0].mirror.url.as_str(),
                    "https://mirror.example.com/channel/"
                );
            }
        }
    }

    #[test]
    fn from_map_preserves_mirrors_for_equivalent_prefixes() {
        let middleware = MirrorMiddleware::from_map(std::collections::HashMap::from([
            (
                "https://upstream.example.com/channel".parse().unwrap(),
                vec![mirror_setting(
                    "https://first.example.com/channel".parse().unwrap(),
                )],
            ),
            (
                "https://upstream.example.com/channel/".parse().unwrap(),
                vec![mirror_setting(
                    "https://second.example.com/channel/".parse().unwrap(),
                )],
            ),
        ]));
        let source: Url = "https://upstream.example.com/channel/".parse().unwrap();
        assert_eq!(middleware.keys(), [(source.to_string(), source.clone())]);
        let mirrors = &middleware.mirror_map[&source];
        assert_eq!(mirrors.len(), 2);
        assert_eq!(
            mirrors[0].mirror.url.as_str(),
            "https://first.example.com/channel/"
        );
        assert_eq!(
            mirrors[1].mirror.url.as_str(),
            "https://second.example.com/channel/"
        );
    }

    #[cfg(feature = "rattler_config")]
    #[test]
    fn from_config_appends_trailing_slashes() {
        let (config, _) = rattler_config::ConfigBase::<rattler_config::NoExtension>::from_toml_str(
            r#"
            [mirrors]
            "https://conda.anaconda.org/conda-forge" = ["https://mirror.example.com/conda-forge"]
            "#,
        )
        .unwrap();

        let middleware = MirrorMiddleware::from_config(&config);
        assert_eq!(
            middleware.keys(),
            [(
                "https://conda.anaconda.org/conda-forge/".to_string(),
                Url::parse("https://conda.anaconda.org/conda-forge/").unwrap()
            )]
        );
        let source: Url = "https://conda.anaconda.org/conda-forge/".parse().unwrap();
        assert_eq!(
            middleware.mirror_map[&source][0].mirror.url.as_str(),
            "https://mirror.example.com/conda-forge/"
        );
    }
}
