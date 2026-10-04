use rattler_config::{Config as _, ConfigBase};
use wasm_bindgen::prelude::*;

use crate::{JsError, JsResult};

/// The shared rattler configuration, as read from a `config.toml` by
/// rattler-based tools.
///
/// There is no file system in the browser, so a configuration is parsed from
/// a TOML string with {@link Config.fromToml} instead of being loaded from the
/// default locations.
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

fn parse_toml(toml: &str, shared: Option<bool>) -> JsResult<(ConfigBase, Vec<String>)> {
    let (config, unused) = if shared.unwrap_or(false) {
        ConfigBase::from_toml_str_shared(toml)
    } else {
        ConfigBase::from_toml_str(toml)
    }
    .map_err(|err| JsError::ParseConfig(err.to_string()))?;
    Ok((config, unused.into_iter().collect()))
}

#[wasm_bindgen(js_class = "Config")]
impl JsConfig {
    /// Creates a configuration with every key at its default.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses a configuration from a TOML string, discarding the keys that
    /// were not recognized. Use {@link Config.fromTomlWithUnusedKeys} to
    /// inspect them.
    ///
    /// When `shared` is `true` the string is parsed as a *shared*
    /// configuration file: only the keys shared by all rattler-based tools
    /// are accepted.
    #[wasm_bindgen(js_name = "fromToml")]
    pub fn from_toml(
        #[wasm_bindgen(param_description = "The TOML document to parse")] toml: &str,
        #[wasm_bindgen(
            param_description = "Whether to parse the document as a shared configuration file"
        )]
        shared: Option<bool>,
    ) -> JsResult<JsConfig> {
        Ok(parse_toml(toml, shared)?.0.into())
    }

    /// Parses a configuration from a TOML string and returns it together
    /// with the sorted keys that were not recognized.
    ///
    /// When `shared` is `true` the string is parsed as a *shared*
    /// configuration file: only the keys shared by all rattler-based tools
    /// are accepted.
    #[wasm_bindgen(
        js_name = "fromTomlWithUnusedKeys",
        unchecked_return_type = "{ config: Config; unusedKeys: string[] }"
    )]
    pub fn from_toml_with_unused_keys(
        #[wasm_bindgen(param_description = "The TOML document to parse")] toml: &str,
        #[wasm_bindgen(
            param_description = "Whether to parse the document as a shared configuration file"
        )]
        shared: Option<bool>,
    ) -> JsResult<JsValue> {
        let (config, unused) = parse_toml(toml, shared)?;

        let result = js_sys::Object::new();
        js_sys::Reflect::set(
            &result,
            &JsValue::from_str("config"),
            &JsValue::from(JsConfig::from(config)),
        )
        .expect("setting a property on a plain object cannot fail");
        js_sys::Reflect::set(
            &result,
            &JsValue::from_str("unusedKeys"),
            &serde_wasm_bindgen::to_value(&unused)?,
        )
        .expect("setting a property on a plain object cannot fail");
        Ok(result.into())
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
