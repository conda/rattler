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
}

impl GatewayBuilder {
    /// New instance of the builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the client to use for fetching repodata.
    #[must_use]
    pub fn with_client(mut self, client: ClientWithMiddleware) -> Self {
        self.client = Some(client);
        self
    }

    /// Set the channel configuration to use for fetching repodata.
    #[must_use]
    pub fn with_channel_config(mut self, channel_config: ChannelConfig) -> Self {
        self.channel_config = channel_config;
        self
    }

    /// Set the directory to use for caching repodata.
    #[must_use]
    pub fn with_cache_dir(mut self, cache: impl Into<PathBuf>) -> Self {
        self.cache = Some(cache.into());
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

        Gateway {
            inner: Arc::new(GatewayInner {
                subdirs: DashMap::default(),
                client,
                channel_config: self.channel_config,
                cache,
            }),
        }
    }
}
