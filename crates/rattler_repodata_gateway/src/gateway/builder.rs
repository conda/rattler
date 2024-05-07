use crate::gateway::GatewayInner;
use crate::{ChannelConfig, Gateway};
use dashmap::DashMap;
use reqwest::Client;
use reqwest_middleware::ClientWithMiddleware;
use std::path::PathBuf;
use std::sync::Arc;

/// A builder for constructing a [`Gateway`].
#[derive(Default)]
pub struct GatewayBuilder {
    channel_config: ChannelConfig,
    client: Option<ClientWithMiddleware>,
    cache: Option<PathBuf>,
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
    #[must_use]
    pub fn with_cache_dir(mut self, cache: impl Into<PathBuf>) -> Self {
        self.set_cache_dir(cache);
        self
    }

    /// Set the directory to use for caching repodata.
    pub fn set_cache_dir(&mut self, cache: impl Into<PathBuf>) -> &mut Self {
        self.cache = Some(cache.into());
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

        let cache = self.cache.unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("rattler/cache")
        });

        let max_concurrent_requests = self.max_concurrent_requests.unwrap_or(100);
        Gateway {
            inner: Arc::new(GatewayInner {
                subdirs: DashMap::default(),
                client,
                channel_config: self.channel_config,
                cache,
                concurrent_requests_semaphore: Arc::new(tokio::sync::Semaphore::new(
                    max_concurrent_requests,
                )),
            }),
        }
    }
}
