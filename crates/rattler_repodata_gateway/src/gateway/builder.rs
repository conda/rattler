use crate::gateway::GatewayInner;
use crate::{ChannelConfig, Gateway};
use dashmap::DashMap;
#[cfg(not(target_arch = "wasm32"))]
use rattler_cache::package_cache::PackageCache;
use reqwest::Client;
use reqwest_middleware::ClientWithMiddleware;
use std::sync::Arc;

/// A builder for constructing a [`Gateway`].
#[derive(Default)]
pub struct GatewayBuilder {
    channel_config: ChannelConfig,
    client: Option<ClientWithMiddleware>,
    #[cfg(not(target_arch = "wasm32"))]
    cache: Option<std::path::PathBuf>,
    #[cfg(not(target_arch = "wasm32"))]
    package_cache: Option<PackageCache>,
    max_concurrent_requests: Option<usize>,
}

impl GatewayBuilder {
    /// New instance of the builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the client to use for fetching repodata.
    #[must_use]
    pub fn with_client(mut self, client: ClientWithMiddleware) -> Self {
        self.set_client(client);
        self
    }

    /// Set the client to use for fetching repodata.
    pub fn set_client(&mut self, client: ClientWithMiddleware) -> &mut Self {
        self.client = Some(client);
        self
    }

    /// Set the channel configuration to use for fetching repodata.
    #[must_use]
    pub fn with_channel_config(mut self, channel_config: ChannelConfig) -> Self {
        self.set_channel_config(channel_config);
        self
    }

    /// Sets the channel configuration to use for fetching repodata.
    pub fn set_channel_config(&mut self, channel_config: ChannelConfig) -> &mut Self {
        self.channel_config = channel_config;
        self
    }

    /// Set the directory to use for caching repodata.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn with_cache_dir(mut self, cache: impl Into<std::path::PathBuf>) -> Self {
        self.set_cache_dir(cache);
        self
    }

    /// Add package cache to the builder to store packages.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_package_cache(mut self, package_cache: PackageCache) -> Self {
        self.set_package_cache(package_cache);
        self
    }

    /// Set the directory to use for caching repodata.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_cache_dir(&mut self, cache: impl Into<std::path::PathBuf>) -> &mut Self {
        self.cache = Some(cache.into());
        self
    }

    /// Set the directory to use for caching packages.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_package_cache(&mut self, package_cache: PackageCache) -> &mut Self {
        self.package_cache = Some(package_cache);
        self
    }

    /// Sets the maximum number of concurrent HTTP requests to make.
    #[must_use]
    pub fn with_max_concurrent_requests(mut self, max_concurrent_requests: usize) -> Self {
        self.set_max_concurrent_requests(max_concurrent_requests);
        self
    }

    /// Sets the maximum number of concurrent HTTP requests to make.
    pub fn set_max_concurrent_requests(&mut self, max_concurrent_requests: usize) -> &mut Self {
        self.max_concurrent_requests = Some(max_concurrent_requests);
        self
    }

    /// Finish the construction of the gateway returning a constructed gateway.
    pub fn finish(self) -> Gateway {
        let client = self
            .client
            .unwrap_or_else(|| ClientWithMiddleware::from(Client::new()));

        #[cfg(not(target_arch = "wasm32"))]
        let cache = self.cache.unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("rattler/cache")
        });

        #[cfg(not(target_arch = "wasm32"))]
        let package_cache = self.package_cache.unwrap_or(PackageCache::new(
            cache.join(rattler_cache::PACKAGE_CACHE_DIR),
        ));

        let max_concurrent_requests = self.max_concurrent_requests.unwrap_or(100);
        Gateway {
            inner: Arc::new(GatewayInner {
                subdirs: DashMap::default(),
                client,
                channel_config: self.channel_config,
                #[cfg(not(target_arch = "wasm32"))]
                cache,
                #[cfg(not(target_arch = "wasm32"))]
                package_cache,
                concurrent_requests_semaphore: Arc::new(tokio::sync::Semaphore::new(
                    max_concurrent_requests,
                )),
            }),
        }
    }
}
