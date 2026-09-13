use std::{collections::HashMap, path::PathBuf, str::FromStr};

use rattler_conda_types::{
    Channel, ChannelNoticeLevel, MatchSpec, ParseMatchSpecOptions, Platform, RepoDataRecord,
    RepodataRevision,
};
use rattler_repodata_gateway::{
    ChannelConfig, Gateway, GatewayWarning, SourceConfig, fetch::CacheAction,
};
use reqwest::Client;
use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Serialize};
use url::Url;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
unsafe extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = warn)]
    fn console_warn(s: &str);
}

/// Forward each [`GatewayWarning`] to JS's `console.warn`. CEP-42's
/// default `Warn` mode produces non-fatal warnings that the Rust API
/// surfaces on the query output; for the JS binding we forward them
/// to the host's standard warnings channel so they cannot be
/// silently lost.
pub(crate) fn emit_gateway_warnings(warnings: Vec<GatewayWarning>) {
    for w in warnings {
        console_warn(&w.to_string());
    }
}

use crate::JsResult;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Notice {
    channel: String,
    id: String,
    message: String,
    level: &'static str,
    created_at: Option<String>,
    expires_at: Option<String>,
    interval: Option<u64>,
}

impl From<rattler_repodata_gateway::ChannelNoticeResult> for Notice {
    fn from(result: rattler_repodata_gateway::ChannelNoticeResult) -> Self {
        Self {
            channel: result.channel.to_string(),
            id: result.notice.id,
            message: result.notice.message,
            level: match result.notice.level {
                ChannelNoticeLevel::Info => "info",
                ChannelNoticeLevel::Warning => "warning",
                ChannelNoticeLevel::Critical => "critical",
            },
            created_at: result
                .notice
                .created_at
                .map(|timestamp| timestamp.to_string()),
            expires_at: result
                .notice
                .expires_at
                .map(|timestamp| timestamp.to_string()),
            interval: result.notice.interval,
        }
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct JsGateway {
    inner: Gateway,
    on_warning: Option<js_sys::Function>,
}

impl From<Gateway> for JsGateway {
    fn from(value: Gateway) -> Self {
        JsGateway {
            inner: value,
            on_warning: None,
        }
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
        // Spread the rest, so a new `SourceConfig` field does not break this
        // binding; the ones not listed here are simply not exposed to JS.
        Self {
            zstd_enabled: value.zstd_enabled,
            bz2_enabled: value.bz2_enabled,
            sharded_enabled: value.sharded_enabled,
            cache_action: CacheAction::default(),
            ..SourceConfig::default()
        }
    }
}

#[wasm_bindgen]
impl JsGateway {
    #[wasm_bindgen(constructor)]
    pub fn new(
        input: JsValue,
        #[wasm_bindgen(param_description = "A custom fetch implementation used for all requests")]
        fetch: Option<js_sys::Function>,
        #[wasm_bindgen(param_description = "A callback invoked for every gateway warning")]
        on_warning: Option<js_sys::Function>,
    ) -> JsResult<Self> {
        let options: Option<JsGatewayOptions> = serde_wasm_bindgen::from_value(input)?;

        // Creating the Gateway with a default client to avoid adding a user-agent header
        // (Not supported from the browser)
        let mut builder = Gateway::builder().with_client(ClientWithMiddleware::from(Client::new()));
        if let Some(fetch) = fetch {
            builder.set_js_fetch(fetch);
        }
        if let Some(options) = options {
            if let Some(max_concurrent_requests) = options.max_concurrent_requests {
                builder.set_max_concurrent_requests(max_concurrent_requests);
            }
            builder.set_channel_config(options.channel_config.into());
        };

        Ok(Self {
            inner: builder.finish(),
            on_warning,
        })
    }

    pub async fn channel_notices(&self, channels: Vec<String>) -> Result<JsValue, JsError> {
        let channel_config =
            rattler_conda_types::ChannelConfig::default_with_root_dir(PathBuf::from(""));
        let channels = channels
            .into_iter()
            .map(|channel| Channel::from_str(&channel, &channel_config))
            .collect::<Result<Vec<_>, _>>()?;
        let notices: Vec<_> = self
            .inner
            .channel_notices(channels.iter())
            .await
            .into_iter()
            .map(Notice::from)
            .collect();
        Ok(serde_wasm_bindgen::to_value(&notices)?)
    }

    /// Forward each [`GatewayWarning`] to the configured warning callback,
    /// or to JS's `console.warn` when none is set. CEP-42's default `Warn`
    /// mode produces non-fatal warnings that the Rust API surfaces on the
    /// query output; forwarding them ensures they cannot be silently lost.
    fn emit_warnings(&self, warnings: Vec<GatewayWarning>) {
        for warning in warnings {
            let message = warning.to_string();
            match &self.on_warning {
                Some(callback) => {
                    let _ = callback.call1(&JsValue::NULL, &JsValue::from_str(&message));
                }
                None => console_warn(&message),
            }
        }
    }

    pub async fn names(
        &self,
        channels: Vec<String>,
        platforms: Vec<String>,
        channel_notices: bool,
    ) -> JsResult<JsValue> {
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

        let output = self
            .inner
            .names(channels, platforms)
            .channel_notices(channel_notices)
            .execute()
            .await?;
        self.emit_warnings(output.warnings);

        #[derive(Serialize)]
        struct NamesOutput {
            names: Vec<String>,
            notices: Vec<Notice>,
        }

        Ok(serde_wasm_bindgen::to_value(&NamesOutput {
            names: output
                .names
                .into_iter()
                .map(|name| name.as_source().to_string())
                .collect(),
            notices: output.notices.into_iter().map(Notice::from).collect(),
        })?)
    }

    /// Queries the given channels and platforms for records matching the
    /// given match specs. Returns the matching records as plain objects in
    /// the same shape as they appear in `repodata.json`, extended with the
    /// `fn`, `url` and `channel` fields, together with any non-fatal
    /// warnings encountered during the query.
    pub async fn query(
        &self,
        channels: Vec<String>,
        platforms: Vec<String>,
        specs: Vec<String>,
        #[wasm_bindgen(
            param_description = "Whether to recursively fetch the records of dependencies as well"
        )]
        recursive: bool,
    ) -> JsResult<JsValue> {
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
        let specs = specs
            .into_iter()
            .map(|s| {
                MatchSpec::from_str(
                    &s,
                    ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;

        let output = self
            .inner
            .query(channels, platforms, specs)
            .recursive(recursive)
            .execute()
            .await?;
        let warnings = output
            .warnings
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        self.emit_warnings(output.warnings);

        #[derive(Serialize)]
        struct QueryOutput<'a> {
            records: Vec<&'a RepoDataRecord>,
            warnings: Vec<String>,
        }

        let records = output
            .repodata
            .iter()
            .flat_map(|repodata| repodata.iter())
            .collect::<Vec<_>>();
        let serializer = serde_wasm_bindgen::Serializer::json_compatible();
        Ok(QueryOutput { records, warnings }.serialize(&serializer)?)
    }
}
