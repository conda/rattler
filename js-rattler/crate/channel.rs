use std::path::PathBuf;

use rattler_conda_types::{Channel, ChannelConfig, Subdir};
use serde::Deserialize;
use url::Url;
use wasm_bindgen::prelude::*;

use crate::{JsError, JsResult, platform::JsPlatform};

#[wasm_bindgen(typescript_custom_section)]
const CHANNEL_OPTIONS_TS: &'static str = r#"
/**
 * Options that control how a `Channel` is parsed.
 *
 * @public
 */
export type ChannelOptions = {
    /**
     * The url that channel names are resolved against. Defaults to
     * `https://conda.anaconda.org/`.
     */
    channelAlias?: string;
};
"#;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(typescript_type = "ChannelOptions")]
    pub type JsChannelOptions;
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChannelOptions {
    #[serde(default)]
    channel_alias: Option<Url>,
}

/// A conda channel, parsed from a channel name (`conda-forge`), a url
/// (`https://prefix.dev/conda-forge`) or a name with explicit platforms
/// (`conda-forge[linux-64,noarch]`).
///
/// @public
#[wasm_bindgen(js_name = "Channel")]
#[repr(transparent)]
#[derive(Clone, Eq, PartialEq)]
pub struct JsChannel {
    inner: Channel,
}

impl From<Channel> for JsChannel {
    fn from(value: Channel) -> Self {
        JsChannel { inner: value }
    }
}

impl From<JsChannel> for Channel {
    fn from(value: JsChannel) -> Self {
        value.inner
    }
}

impl AsRef<Channel> for JsChannel {
    fn as_ref(&self) -> &Channel {
        &self.inner
    }
}

#[wasm_bindgen(js_class = "Channel")]
impl JsChannel {
    /// Parses a channel from a channel name or url.
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(param_description = "The channel name or url.")] channel: &str,
        #[wasm_bindgen(param_description = "Options that control how the channel is parsed.")]
        options: Option<JsChannelOptions>,
    ) -> JsResult<Self> {
        let options: Option<ChannelOptions> = match options {
            Some(options) => serde_wasm_bindgen::from_value(options.into())?,
            None => None,
        };
        let mut config = ChannelConfig::default_with_root_dir(PathBuf::from(""));
        if let Some(channel_alias) = options.unwrap_or_default().channel_alias {
            config.channel_alias = channel_alias;
        }
        Ok(Channel::from_str(channel, &config)?.into())
    }

    /// The name of the channel, if it has one. Channels that are referred to
    /// by a url outside of the channel alias have no name.
    #[wasm_bindgen(getter)]
    pub fn name(&self) -> Option<String> {
        self.inner.name.clone()
    }

    /// The base url of the channel, always ending with a slash.
    #[wasm_bindgen(getter, js_name = "baseUrl")]
    pub fn base_url(&self) -> String {
        self.inner.base_url.to_string()
    }

    /// The base url of the channel with any credentials redacted.
    #[wasm_bindgen(getter, js_name = "canonicalName")]
    pub fn canonical_name(&self) -> String {
        self.inner.canonical_name()
    }

    /// The platforms explicitly selected in the channel string, e.g.
    /// `conda-forge[linux-64,noarch]`.
    #[wasm_bindgen(getter, unchecked_return_type = "Platform[] | undefined")]
    pub fn platforms(&self) -> Option<Vec<String>> {
        self.inner
            .platforms
            .as_ref()
            .map(|platforms| platforms.iter().map(ToString::to_string).collect())
    }

    /// Returns the url of the given platform's subdirectory of the channel.
    #[wasm_bindgen(js_name = "platformUrl")]
    pub fn platform_url(
        &self,
        #[wasm_bindgen(param_description = "The platform")] platform: JsPlatform,
    ) -> JsResult<String> {
        let platform: String = serde_wasm_bindgen::from_value(platform.into())?;
        let platform: Subdir = platform.parse().map_err(JsError::from)?;
        Ok(self.inner.platform_url(platform).to_string())
    }

    /// Returns the canonical name of the channel.
    #[wasm_bindgen(js_name = "toString")]
    pub fn as_str(&self) -> String {
        self.inner.canonical_name()
    }
}
