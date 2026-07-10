//! A lazily-initialized HTTP client.
//!
//! This mirrors `rattler_networking::LazyClient` but lives in this crate so
//! `rattler_git` stays dependency-light. Consumers that already hold a
//! `rattler_networking::LazyClient` (or any other lazily-constructed client)
//! can bridge it with [`LazyClient::new`] and a closure.

use std::sync::Arc;

/// Initialization of the reqwest client can be expensive, using this struct
/// allows creating the client only when needed — for git operations that is
/// only when the GitHub fast path is attempted.
///
/// Clones of this struct will share the same underlying client.
#[derive(Clone)]
pub struct LazyClient {
    initializer: Arc<
        std::sync::LazyLock<
            reqwest_middleware::ClientWithMiddleware,
            Box<dyn FnOnce() -> reqwest_middleware::ClientWithMiddleware + Send + Sync>,
        >,
    >,
}

impl std::fmt::Debug for LazyClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyClient").finish_non_exhaustive()
    }
}

impl Default for LazyClient {
    fn default() -> Self {
        reqwest_middleware::ClientWithMiddleware::default().into()
    }
}

impl From<reqwest_middleware::ClientWithMiddleware> for LazyClient {
    fn from(client: reqwest_middleware::ClientWithMiddleware) -> Self {
        let result = Self {
            initializer: Arc::new(std::sync::LazyLock::new(Box::new(move || client))),
        };
        std::sync::LazyLock::force(&result.initializer);
        result
    }
}

impl From<reqwest::Client> for LazyClient {
    fn from(client: reqwest::Client) -> Self {
        reqwest_middleware::ClientWithMiddleware::from(client).into()
    }
}

impl LazyClient {
    /// Construct a new lazy client from the given initializer function.
    pub fn new<F: FnOnce() -> reqwest_middleware::ClientWithMiddleware + Send + Sync + 'static>(
        f: F,
    ) -> Self {
        Self {
            initializer: Arc::new(std::sync::LazyLock::new(Box::new(f))),
        }
    }

    /// Returns the initialized reqwest client. This will initialize the client
    /// on the first request.
    pub fn client(&self) -> &reqwest_middleware::ClientWithMiddleware {
        &self.initializer
    }
}
