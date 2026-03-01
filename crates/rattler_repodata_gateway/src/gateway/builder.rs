use std::sync::Arc;

use coalesced_map::CoalescedMap;
#[cfg(not(target_arch = "wasm32"))]
use rattler_cache::package_cache::PackageCache;
use rattler_networking::LazyClient;
use reqwest::Client;
use reqwest_middleware::ClientWithMiddleware;

use crate::{gateway::GatewayInner, ChannelConfig, Gateway};

static USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);

/// Defines the maximum concurrency for the gateway.
#[derive(Default, Clone)]
pub enum MaxConcurrency {
    /// No limit on the number of concurrent requests.
    #[default]
    Unlimited,
    /// A specific number of concurrent requests.
    Limited(usize),
    /// Use the specified semaphore for concurrency control.
    Semaphore(Arc<tokio::sync::Semaphore>),
}

impl From<usize> for MaxConcurrency {
    fn from(value: usize) -> Self {
        if value == 0 {
            MaxConcurrency::Unlimited
        } else {
            MaxConcurrency::Limited(value)
        }
    }
}

impl From<Arc<tokio::sync::Semaphore>> for MaxConcurrency {
    fn from(value: Arc<tokio::sync::Semaphore>) -> Self {
        MaxConcurrency::Semaphore(value)
    }
}

/// A builder for constructing a [`Gateway`].
#[derive(Default, Clone)]
pub struct GatewayBuilder {
    channel_config: ChannelConfig,
    client: Option<LazyClient>,
    #[cfg(not(target_arch = "wasm32"))]
    cache: Option<std::path::PathBuf>,
    #[cfg(not(target_arch = "wasm32"))]
    package_cache: Option<PackageCache>,
    max_concurrent_requests: MaxConcurrency,
}

impl GatewayBuilder {
    /// New instance of the builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the client to use for fetching repodata.
    #[must_use]
    pub fn with_client(mut self, client: impl Into<LazyClient>) -> Self {
        self.set_client(client);
        self
    }

    /// Set the client to use for fetching repodata.
    pub fn set_client(&mut self, client: impl Into<LazyClient>) -> &mut Self {
        self.client = Some(client.into());
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
    pub fn with_max_concurrent_requests(
        self,
        max_concurrent_requests: impl Into<MaxConcurrency>,
    ) -> Self {
        Self {
            max_concurrent_requests: max_concurrent_requests.into(),
            ..self
        }
    }

    /// Sets the maximum number of concurrent HTTP requests to make.
    pub fn set_max_concurrent_requests(
        &mut self,
        max_concurrent_requests: impl Into<MaxConcurrency>,
    ) -> &mut Self {
        self.max_concurrent_requests = max_concurrent_requests.into();
        self
    }


    /// Finish the construction of the gateway returning a constructed gateway.
    pub fn finish(self) -> Gateway {
        let client = self.client.unwrap_or_else(|| {
            LazyClient::new(move || {
                let client_builder = Client::builder().user_agent(USER_AGENT);
                ClientWithMiddleware::from(client_builder.build().unwrap())
            })
        });

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

        let concurrent_requests_semaphore = match self.max_concurrent_requests {
            MaxConcurrency::Unlimited => None,
            MaxConcurrency::Limited(n) => Some(Arc::new(tokio::sync::Semaphore::new(n))),
            MaxConcurrency::Semaphore(sem) => Some(sem),
        };

        Gateway {
            inner: Arc::new(GatewayInner {
                subdirs: CoalescedMap::new(),
                client,
                channel_config: self.channel_config,
                #[cfg(not(target_arch = "wasm32"))]
                cache,
                #[cfg(not(target_arch = "wasm32"))]
                package_cache,
                subdir_run_exports_cache: Arc::default(),
                concurrent_requests_semaphore,
            }),
        }
    }
}
