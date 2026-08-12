use std::{
    collections::HashMap,
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex},
};

use rattler_conda_types::{Channel, ChannelNoticeLevel, Platform};
use rattler_repodata_gateway::{
    ChannelConfig, DownloadReporter, Gateway, GatewayWarning, Reporter, SourceConfig,
    UnsupportedRepodataRevision, fetch::CacheAction,
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

/// A non-fatal repodata layout revision advertised by a queried channel.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UnsupportedRepodataRevisionReport {
    channel: String,
    subdir: String,
    supported_revision: String,
    advertised_revision: String,
    message: Option<String>,
}

impl From<UnsupportedRepodataRevision> for UnsupportedRepodataRevisionReport {
    fn from(value: UnsupportedRepodataRevision) -> Self {
        Self {
            channel: value.channel,
            subdir: value.subdir,
            supported_revision: value.supported_revision.to_string(),
            advertised_revision: value.revision.to_string(),
            message: value.metadata.message,
        }
    }
}

/// Collect reports emitted by the Rust gateway so they can be returned as
/// query metadata instead of being lost at the wasm boundary.
#[derive(Clone, Default)]
struct UnsupportedRepodataRevisionCollector(Arc<Mutex<Vec<UnsupportedRepodataRevision>>>);

impl UnsupportedRepodataRevisionCollector {
    fn into_reports(self) -> Vec<UnsupportedRepodataRevisionReport> {
        std::mem::take(&mut *self.0.lock().expect("revision collector poisoned"))
            .into_iter()
            .map(UnsupportedRepodataRevisionReport::from)
            .collect()
    }
}

impl Reporter for UnsupportedRepodataRevisionCollector {
    fn download_reporter(&self) -> Option<&dyn DownloadReporter> {
        None
    }

    fn on_unsupported_repodata_revision(&self, report: &UnsupportedRepodataRevision) {
        self.0
            .lock()
            .expect("revision collector poisoned")
            .push(report.clone());
    }
}

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
            inner: builder.finish(),
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

    pub async fn names(
        &self,
        channels: Vec<String>,
        platforms: Vec<String>,
        channel_notices: bool,
    ) -> Result<JsValue, JsError> {
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

        let reporter = UnsupportedRepodataRevisionCollector::default();
        let output = self
            .inner
            .names(channels, platforms)
            .channel_notices(channel_notices)
            .with_reporter(reporter.clone())
            .execute()
            .await?;
        emit_gateway_warnings(output.warnings);

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct NamesOutput {
            names: Vec<String>,
            notices: Vec<Notice>,
            unsupported_repodata_revisions: Vec<UnsupportedRepodataRevisionReport>,
        }

        Ok(serde_wasm_bindgen::to_value(&NamesOutput {
            names: output
                .names
                .into_iter()
                .map(|name| name.as_source().to_string())
                .collect(),
            notices: output.notices.into_iter().map(Notice::from).collect(),
            unsupported_repodata_revisions: reporter.into_reports(),
        })?)
    }
}

#[cfg(test)]
mod tests {
    use rattler_conda_types::{RepodataRevision, RepodataRevisionMetadata};

    use super::{Reporter, UnsupportedRepodataRevision, UnsupportedRepodataRevisionCollector};

    fn report(
        channel: &str,
        subdir: &str,
        revision: RepodataRevision,
        message: Option<&str>,
    ) -> UnsupportedRepodataRevision {
        UnsupportedRepodataRevision {
            channel: channel.to_string(),
            subdir: subdir.to_string(),
            supported_revision: RepodataRevision::V3,
            revision,
            metadata: RepodataRevisionMetadata {
                message: message.map(str::to_string),
                ..Default::default()
            },
        }
    }

    #[test]
    fn collects_revision_reports_without_losing_their_metadata() {
        let collector = UnsupportedRepodataRevisionCollector::default();
        collector.on_unsupported_repodata_revision(&report(
            "https://example.com/first/",
            "linux-64",
            RepodataRevision::Unknown(1),
            None,
        ));
        collector.on_unsupported_repodata_revision(&report(
            "https://example.com/second/",
            "noarch",
            RepodataRevision::from(4),
            Some("new layout"),
        ));

        let reports = collector.into_reports();
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].channel, "https://example.com/first/");
        assert_eq!(reports[0].subdir, "linux-64");
        assert_eq!(reports[0].supported_revision, "v3");
        assert_eq!(reports[0].advertised_revision, "v1");
        assert_eq!(reports[0].message, None);
        assert_eq!(reports[1].channel, "https://example.com/second/");
        assert_eq!(reports[1].subdir, "noarch");
        assert_eq!(reports[1].advertised_revision, "v4");
        assert_eq!(reports[1].message.as_deref(), Some("new layout"));
    }
}
