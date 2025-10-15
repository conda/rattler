use std::{collections::HashMap, path::PathBuf, str::FromStr};

use rattler_conda_types::{Channel, Platform};
use rattler_repodata_gateway::{fetch::CacheAction, ChannelConfig, Gateway, SourceConfig};
use reqwest::Client;
use reqwest_middleware::ClientWithMiddleware;
use serde::Deserialize;
use url::Url;
use wasm_bindgen::prelude::*;

use crate::JsResult;

#[wasm_bindgen]
#[repr(transparent)]
#[derive(Clone)]
pub struct JsGateway {
    inner: Gateway,
}

impl From<Gateway> for JsGateway {
    fn from(value: Gateway) -> Self {
        JsGateway { inner: value }
    }
}

impl From<JsGateway> for Gateway {
    fn from(value: JsGateway) -> Self {
        value.inner
    }
}

impl AsRef<Gateway> for JsGateway {
    fn as_ref(&self) -> &Gateway {
        &self.inner
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsGatewayOptions {
    max_concurrent_requests: Option<usize>,

    #[serde(default)]
    channel_config: JsChannelConfig,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsChannelConfig {
    #[serde(default)]
    default: JsSourceConfig,
    #[serde(default)]
    per_channel: HashMap<Url, JsSourceConfig>,
}

impl From<JsChannelConfig> for ChannelConfig {
    fn from(value: JsChannelConfig) -> Self {
        ChannelConfig {
            default: value.default.into(),
            per_channel: value
                .per_channel
                .into_iter()
                .map(|(key, value)| (key, value.into()))
                .collect(),
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsSourceConfig {
    #[serde(default = "yes")]
    zstd_enabled: bool,

    #[serde(default = "yes")]
    bz2_enabled: bool,

    #[serde(default = "yes")]
    sharded_enabled: bool,
}

impl Default for JsSourceConfig {
    fn default() -> Self {
        Self {
            zstd_enabled: true,
            bz2_enabled: true,
            sharded_enabled: true,
        }
    }
}

impl From<JsSourceConfig> for SourceConfig {
    fn from(value: JsSourceConfig) -> Self {
        Self {
            jlap_enabled: false,
            zstd_enabled: value.zstd_enabled,
            bz2_enabled: value.bz2_enabled,
            sharded_enabled: value.sharded_enabled,
            cache_action: CacheAction::default(),
        }
    }
}

#[wasm_bindgen]
impl JsGateway {
    #[wasm_bindgen(constructor)]
    pub fn new(input: JsValue) -> JsResult<Self> {
        // Creating the Gateway with a default client to avoid adding a user-agent header
        // (Not supported from the browser)
        let mut builder = Gateway::builder().with_client(ClientWithMiddleware::from(Client::new()));
        let options: Option<JsGatewayOptions> = serde_wasm_bindgen::from_value(input)?;
        if let Some(options) = options {
            if let Some(max_concurrent_requests) = options.max_concurrent_requests {
                builder.set_max_concurrent_requests(max_concurrent_requests);
            }
            builder.set_channel_config(options.channel_config.into());
        };

        Ok(Self {
            inner: builder.finish_with_user_agent(false),
        })
    }

    pub async fn names(
        &self,
        channels: Vec<String>,
        platforms: Vec<String>,
    ) -> Result<Vec<String>, JsError> {
        // TODO: Dont hardcode
        let channel_config =
            rattler_conda_types::ChannelConfig::default_with_root_dir(PathBuf::from(""));

        let channels = channels
            .into_iter()
            .map(|s| Channel::from_str(&s, &channel_config))
            .collect::<Result<Vec<_>, _>>()?;
        let platforms = platforms
            .into_iter()
            .map(|p| Platform::from_str(&p))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(self
            .inner
            .names(channels, platforms)
            .execute()
            .await?
            .into_iter()
            .map(|name| name.as_source().to_string())
            .collect())
    }
}
