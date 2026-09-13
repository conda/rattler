//! Middleware that rejects network requests when offline mode is enabled.

use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next};

/// Middleware that immediately rejects every outgoing request.
///
/// This can be installed as the first middleware in a client stack to ensure
/// that offline mode prevents all network access before authentication,
/// transport-specific URL rewriting, or retry middleware can run.
#[derive(Clone, Copy, Debug, Default)]
pub struct OfflineMiddleware;

/// Error returned when [`OfflineMiddleware`] blocks a request.
#[derive(Debug, thiserror::Error)]
#[error("network access is disabled by offline mode")]
pub struct OfflineError;

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Middleware for OfflineMiddleware {
    async fn handle(
        &self,
        req: Request,
        _extensions: &mut http::Extensions,
        _next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        tracing::debug!(url = %req.url(), "blocking request because offline mode is enabled");
        Err(reqwest_middleware::Error::Middleware(OfflineError.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::OfflineMiddleware;

    #[tokio::test]
    async fn rejects_requests_before_network_access() {
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
            .with(OfflineMiddleware)
            .build();

        let err = client
            .get("https://example.com/repodata.json")
            .send()
            .await
            .expect_err("offline middleware should reject the request");

        assert!(
            err.to_string().contains("offline mode"),
            "unexpected error: {err}"
        );
    }
}
