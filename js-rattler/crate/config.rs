use std::collections::HashMap;

use rattler_config::{CommonConfig, Config as _, ConfigBase};
use rattler_repodata_gateway::{ChannelConfig, SourceConfig};
use serde::Serialize;
use wasm_bindgen::prelude::*;

use crate::{JsError, JsResult};

#[wasm_bindgen(typescript_custom_section)]
const CONFIG_JSON_TS: &'static str = r#"
/**
 * The repodata options of the `repodata-config` table, or of one of its
 * per-channel entries.
 *
 * @public
 */
export type RepodataChannelConfigJson = {
    "disable-bzip2"?: boolean;
    "disable-zstd"?: boolean;
    "disable-sharded"?: boolean;
};

/**
 * The `repodata-config` table: the channel-independent options, plus one
 * entry per channel url that overrides them.
 *
 * @public
 */
export type RepodataConfigJson = RepodataChannelConfigJson & {
    [channelUrl: string]: RepodataChannelConfigJson | boolean | undefined;
};

/**
 * The shared rattler configuration as a plain object. The keys are the ones
 * of a rattler `config.toml`, so `[repodata-config]` becomes
 * `"repodata-config"`.
 *
 * @public
 */
export type ConfigJson = {
    /** The channels used when none are specified. */
    "default-channels"?: string[];
    /** Mirrors per channel url, in order of preference. */
    mirrors?: Record<string, string[]>;
    /** Which repodata formats are fetched, globally and per channel. */
    "repodata-config"?: RepodataConfigJson;
    concurrency?: {
        /** The maximum number of concurrent solves. */
        solves?: number;
        /** The maximum number of concurrent HTTP requests. */
        downloads?: number;
    };
    "authentication-override-file"?: string;
    "tls-no-verify"?: boolean;
    "tls-root-certs"?: "webpki" | "system";
    "run-post-link-scripts"?: "insecure" | "false";
    "allow-symbolic-links"?: boolean;
    "allow-hard-links"?: boolean;
    "allow-ref-links"?: boolean;
    build?: Record<string, unknown>;
    "proxy-config"?: Record<string, unknown>;
    "s3-options"?: Record<string, unknown>;
    "index-config"?: Record<string, unknown>;
};

/**
 * The options {@link Config.gatewayOptions} derives from a configuration. They
 * are a subset of the `GatewayOptions` a `Gateway` is constructed with.
 *
 * @public
 */
export type ConfigGatewayOptions = {
    maxConcurrentRequests: number;
    channelConfig: {
        default: ConfigGatewaySourceConfig;
        perChannel: { [channelUrl: string]: ConfigGatewaySourceConfig };
    };
};

/**
 * The repodata formats enabled for a channel, see
 * {@link ConfigGatewayOptions}.
 *
 * @public
 */
export type ConfigGatewaySourceConfig = {
    zstdEnabled: boolean;
    bz2Enabled: boolean;
    shardedEnabled: boolean;
};
"#;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(typescript_type = "ConfigJson")]
    pub type JsConfigJson;

    #[wasm_bindgen(typescript_type = "ConfigGatewayOptions")]
    pub type JsGatewayOptionsJson;

    #[wasm_bindgen(js_namespace = console, js_name = warn)]
    fn console_warn(s: &str);
}

/// The shared rattler configuration, as read from a `config.toml` by
/// rattler-based tools, in the form of a plain object with the same keys.
///
/// @public
#[wasm_bindgen(js_name = "Config")]
#[derive(Clone, Default)]
pub struct JsConfig {
    inner: ConfigBase,
}

impl From<ConfigBase> for JsConfig {
    fn from(value: ConfigBase) -> Self {
        JsConfig { inner: value }
    }
}

impl AsRef<ConfigBase> for JsConfig {
    fn as_ref(&self) -> &ConfigBase {
        &self.inner
    }
}

#[wasm_bindgen(js_class = "Config")]
impl JsConfig {
    /// Creates a configuration from its plain object form. Keys that are not
    /// set keep their default; keys that are not recognized are reported
    /// through `console.warn` and ignored.
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(param_description = "The configuration, defaults when omitted")]
        json: Option<JsConfigJson>,
    ) -> JsResult<JsConfig> {
        let Some(json) = json else {
            return Ok(Self::default());
        };
        // `serde_wasm_bindgen` only looks up the fields a struct declares, so
        // unknown keys would go unnoticed. Going through a JSON value lets
        // `serde_ignored` see every key of the object.
        let json: serde_json::Value = serde_wasm_bindgen::from_value(json.into())?;
        let mut unused = Vec::new();
        let common: CommonConfig =
            serde_ignored::deserialize(json, |path| unused.push(path.to_string()))
                .map_err(|err| JsError::InvalidConfig(err.to_string()))?;
        for key in unused {
            console_warn(&format!("ignoring unknown configuration key '{key}'"));
        }
        Ok(ConfigBase {
            common,
            ..ConfigBase::default()
        }
        .into())
    }

    /// Creates a configuration from its plain object form, as returned by
    /// {@link Config.toJson}. Equivalent to the constructor.
    #[wasm_bindgen(js_name = "fromJson")]
    pub fn from_json(
        #[wasm_bindgen(param_description = "The configuration")] json: JsConfigJson,
    ) -> JsResult<JsConfig> {
        Self::new(Some(json))
    }

    /// Converts this configuration to its plain object form. Keys at their
    /// default are left out.
    #[wasm_bindgen(js_name = "toJson")]
    pub fn to_json(&self) -> JsResult<JsConfigJson> {
        let serializer = serde_wasm_bindgen::Serializer::json_compatible();
        Ok(self.inner.common.serialize(&serializer)?.into())
    }

    /// The options to construct a `Gateway` with this configuration:
    /// `repodata-config` selects the enabled repodata formats (with its
    /// per-channel overrides) and `concurrency.downloads` limits the number
    /// of concurrent requests. Spread the result to add or override
    /// options: `new Gateway({ ...config.gatewayOptions(), fetch })`.
    ///
    /// Requests are always made through `fetch`, so the networking keys
    /// (mirrors, proxies, TLS and authentication) are not part of it.
    #[wasm_bindgen(js_name = "gatewayOptions")]
    pub fn gateway_options(&self) -> JsResult<JsGatewayOptionsJson> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct SourceConfigJson {
            zstd_enabled: bool,
            bz2_enabled: bool,
            sharded_enabled: bool,
        }

        impl From<SourceConfig> for SourceConfigJson {
            fn from(value: SourceConfig) -> Self {
                Self {
                    zstd_enabled: value.zstd_enabled,
                    bz2_enabled: value.bz2_enabled,
                    sharded_enabled: value.sharded_enabled,
                }
            }
        }

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct ChannelConfigJson {
            default: SourceConfigJson,
            per_channel: HashMap<String, SourceConfigJson>,
        }

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct GatewayOptionsJson {
            max_concurrent_requests: usize,
            channel_config: ChannelConfigJson,
        }

        let channel_config = ChannelConfig::from(&self.inner.common);
        let options = GatewayOptionsJson {
            max_concurrent_requests: self.inner.concurrency.downloads,
            channel_config: ChannelConfigJson {
                default: channel_config.default.into(),
                per_channel: channel_config
                    .per_channel
                    .into_iter()
                    .map(|(url, config)| (url.to_string(), config.into()))
                    .collect(),
            },
        };
        let serializer = serde_wasm_bindgen::Serializer::json_compatible();
        Ok(options.serialize(&serializer)?.into())
    }

    /// The channels used when none are specified, as written in the
    /// configuration. `undefined` when the key is not set.
    #[wasm_bindgen(getter, js_name = "defaultChannels")]
    pub fn default_channels(&self) -> Option<Vec<String>> {
        self.inner
            .default_channels
            .as_ref()
            .map(|channels| channels.iter().map(ToString::to_string).collect())
    }

    /// The maximum number of concurrent HTTP requests (`concurrency.downloads`).
    #[wasm_bindgen(getter, js_name = "concurrencyDownloads")]
    pub fn concurrency_downloads(&self) -> usize {
        self.inner.concurrency.downloads
    }

    /// The maximum number of concurrent solves (`concurrency.solves`).
    #[wasm_bindgen(getter, js_name = "concurrencySolves")]
    pub fn concurrency_solves(&self) -> usize {
        self.inner.concurrency.solves
    }

    /// Merges `other` into a copy of this configuration. Keys set in `other`
    /// take precedence.
    pub fn merge(
        &self,
        #[wasm_bindgen(param_description = "The configuration that takes precedence")]
        other: &JsConfig,
    ) -> JsResult<JsConfig> {
        Ok(self
            .inner
            .clone()
            .merge_config(&other.inner)
            .map_err(|err| JsError::InvalidConfig(err.to_string()))?
            .into())
    }

    /// Validates this configuration, throwing when it is invalid.
    pub fn validate(&self) -> JsResult<()> {
        self.inner
            .validate()
            .map_err(|err| JsError::InvalidConfig(err.to_string()))
    }
}
