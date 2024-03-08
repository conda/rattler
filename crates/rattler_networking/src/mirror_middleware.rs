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
    use std::{future::IntoFuture, net::SocketAddr};

    use axum::{extract::State, http::StatusCode, routing::get, Router};
    use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
    use url::Url;

    use crate::MirrorMiddleware;

    async fn count(State(name): State<String>) -> String {
        format!("Hi from counter: {}", name)
    }

    async fn broken_return() -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }

    async fn test_server(name: &str, broken: bool) -> Url {
        let state = String::from(name);

        // Construct a router that returns data from the static dir but fails the first try.
        let router = if !broken {
            Router::new().route("/count", get(count)).with_state(state)
        } else {
            Router::new().route("/count", get(broken_return))
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
            "http://bla.com".to_string(),
            vec![addr_1.to_string(), addr_2.to_string()],
        );

        let middleware = crate::MirrorMiddleware::from_map(mirror_map);
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
            .with(middleware)
            .build();

        let res = client.get("http://bla.com/count").send().await.unwrap();
        assert!(res.status().is_success());
        let res = res.text().await.unwrap();
        println!("{}", res);
        // should always take the first element from the list
        assert!(res == "Hi from counter: server 1")
    }

    #[tokio::test]
    async fn test_mirror_middleware_broken() {
        let addr_1 = test_server("server 1", true).await;
        let addr_2 = test_server("server 2", false).await;

        let mut mirror_map = std::collections::HashMap::new();

        mirror_map.insert(
            "http://bla.com".to_string(),
            vec![addr_1.to_string(), addr_2.to_string()],
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
}
