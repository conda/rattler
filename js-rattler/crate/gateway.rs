use std::{collections::HashMap, path::PathBuf, str::FromStr};

use rattler_conda_types::{
    Channel, ChannelNoticeLevel, GenericVirtualPackage, MatchSpec, PackageName, PackageRecord,
    Subdir, Version,
};
use rattler_repodata_gateway::{
    CacheClearMode, ChannelConfig, Gateway, GatewayWarning, SourceConfig, SubdirSelection,
    fetch::CacheAction,
    who_needs::{DependencyKind, Dependent, RunExportKind, WhoNeedsTarget},
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

use crate::{
    JsResult, match_spec::JsMatchSpec, package_record::JsPackageRecord,
    repo_data_record::JsRepoDataRecord,
};

/// Parses channel names or urls the way every gateway method does.
fn parse_channels(channels: Vec<String>) -> JsResult<Vec<Channel>> {
    // TODO: Dont hardcode
    let channel_config =
        rattler_conda_types::ChannelConfig::default_with_root_dir(PathBuf::from(""));
    Ok(channels
        .into_iter()
        .map(|s| Channel::from_str(&s, &channel_config))
        .collect::<Result<Vec<_>, _>>()?)
}

fn parse_platforms(platforms: Vec<String>) -> JsResult<Vec<Subdir>> {
    Ok(platforms
        .into_iter()
        .map(|p| Subdir::from_str(&p))
        .collect::<Result<Vec<_>, _>>()?)
}

/// Sets `key` on a plain JS object.
fn set_property(object: &js_sys::Object, key: &str, value: &JsValue) {
    js_sys::Reflect::set(object, &JsValue::from_str(key), value)
        .expect("setting a property on a plain object cannot fail");
}

/// Converts a [`Dependent`] into the plain object shape of the TypeScript
/// `Dependent` type: the record, the dependency string, the `kind`, and
/// either `extra` or `runExportKind` depending on the kind.
fn dependent_to_js(dependent: Dependent) -> JsValue {
    let object = js_sys::Object::new();
    set_property(
        &object,
        "record",
        &JsValue::from(JsRepoDataRecord::from(std::sync::Arc::unwrap_or_clone(
            dependent.record,
        ))),
    );
    set_property(
        &object,
        "dependency",
        &JsValue::from_str(&dependent.dependency),
    );
    let kind = match dependent.kind {
        DependencyKind::Depends => "depends",
        DependencyKind::Constrains => "constrains",
        DependencyKind::ExtraDepends(extra) => {
            set_property(&object, "extra", &JsValue::from_str(&extra));
            "extra_depends"
        }
        DependencyKind::RunExport(run_export_kind) => {
            let run_export_kind = match run_export_kind {
                RunExportKind::Weak => "weak",
                RunExportKind::Strong => "strong",
                RunExportKind::Noarch => "noarch",
                RunExportKind::WeakConstrains => "weak_constrains",
                RunExportKind::StrongConstrains => "strong_constrains",
            };
            set_property(
                &object,
                "runExportKind",
                &JsValue::from_str(run_export_kind),
            );
            "run_export"
        }
    };
    set_property(&object, "kind", &JsValue::from_str(kind));
    object.into()
}

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

    /// Clears the in-memory repodata cache of `channel` for the given
    /// platforms, or for every platform when none are given. Subsequent
    /// queries fetch the repodata again.
    #[wasm_bindgen(js_name = "clearRepodataCache")]
    pub fn clear_repodata_cache(
        &self,
        #[wasm_bindgen(param_description = "The channel name or url")] channel: String,
        #[wasm_bindgen(
            param_description = "The platforms to clear, all platforms when omitted",
            unchecked_param_type = "string[] | undefined"
        )]
        platforms: Option<Vec<String>>,
    ) -> JsResult<()> {
        let channel = parse_channels(vec![channel])?
            .pop()
            .expect("one channel in, one channel out");
        let subdirs = match platforms {
            Some(platforms) => SubdirSelection::Some(
                parse_platforms(platforms)?
                    .into_iter()
                    .map(|platform| platform.to_string())
                    .collect(),
            ),
            None => SubdirSelection::All,
        };
        // Only the in-memory cache exists on wasm, so this cannot fail.
        self.inner
            .clear_repodata_cache(&channel, subdirs, CacheClearMode::InMemoryOnly)
            .expect("clearing the in-memory cache cannot fail");
        Ok(())
    }

    /// Finds the records that depend on the package `name`.
    #[wasm_bindgen(js_name = "whoNeedsName", unchecked_return_type = "Promise<unknown[]>")]
    pub fn who_needs_name(
        &self,
        channels: Vec<String>,
        platforms: Vec<String>,
        name: String,
    ) -> JsResult<js_sys::Promise> {
        let name = PackageName::try_from(name)?;
        self.who_needs(channels, platforms, name.into())
    }

    /// Finds the records with a dependency whose match spec matches
    /// `record`.
    #[wasm_bindgen(
        js_name = "whoNeedsRecord",
        unchecked_return_type = "Promise<unknown[]>"
    )]
    pub fn who_needs_record(
        &self,
        channels: Vec<String>,
        platforms: Vec<String>,
        record: &JsPackageRecord,
    ) -> JsResult<js_sys::Promise> {
        let record: &PackageRecord = record.as_ref();
        self.who_needs(channels, platforms, record.clone().into())
    }

    /// Finds the records with a dependency whose match spec matches
    /// `record`.
    #[wasm_bindgen(
        js_name = "whoNeedsRepoDataRecord",
        unchecked_return_type = "Promise<unknown[]>"
    )]
    pub fn who_needs_repo_data_record(
        &self,
        channels: Vec<String>,
        platforms: Vec<String>,
        record: &JsRepoDataRecord,
    ) -> JsResult<js_sys::Promise> {
        let record: &PackageRecord = record.as_ref();
        self.who_needs(channels, platforms, record.clone().into())
    }

    /// Finds the records with a dependency whose match spec matches the
    /// virtual package `name`, `version` and `build_string` (e.g. `__cuda`).
    #[wasm_bindgen(
        js_name = "whoNeedsVirtualPackage",
        unchecked_return_type = "Promise<unknown[]>"
    )]
    pub fn who_needs_virtual_package(
        &self,
        channels: Vec<String>,
        platforms: Vec<String>,
        name: String,
        version: String,
        build_string: String,
    ) -> JsResult<js_sys::Promise> {
        let virtual_package = GenericVirtualPackage {
            name: PackageName::try_from(name)?,
            version: Version::from_str(&version)?,
            build_string,
        };
        self.who_needs(channels, platforms, virtual_package.into())
    }

    pub async fn channel_notices(&self, channels: Vec<String>) -> Result<JsValue, JsError> {
        let channels = parse_channels(channels)?;
        let notices: Vec<_> = self
            .inner
            .channel_notices(channels.iter())
            .await
            .into_iter()
            .map(Notice::from)
            .collect();
        Ok(serde_wasm_bindgen::to_value(&notices)?)
    }

    /// Runs a reverse dependency query. The inputs are parsed synchronously,
    /// so the borrowed target can be cloned before the returned promise
    /// outlives the call.
    fn who_needs(
        &self,
        channels: Vec<String>,
        platforms: Vec<String>,
        target: WhoNeedsTarget,
    ) -> JsResult<js_sys::Promise> {
        let channels = parse_channels(channels)?;
        let platforms = parse_platforms(platforms)?;
        let query = self.inner.who_needs(channels, platforms, target);
        Ok(wasm_bindgen_futures::future_to_promise(async move {
            let dependents = query.execute().await.map_err(JsError::from)?;
            Ok(dependents
                .into_iter()
                .map(dependent_to_js)
                .collect::<js_sys::Array>()
                .into())
        }))
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
        let channels = parse_channels(channels)?;
        let platforms = parse_platforms(platforms)?;

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
        #[wasm_bindgen(
            param_description = "The match specs to query for. They are consumed by the call."
        )]
        specs: Vec<JsMatchSpec>,
        #[wasm_bindgen(
            param_description = "Whether to recursively fetch the records of dependencies as well"
        )]
        recursive: bool,
    ) -> JsResult<JsValue> {
        let channels = parse_channels(channels)?;
        let platforms = parse_platforms(platforms)?;
        let specs = specs.into_iter().map(MatchSpec::from).collect::<Vec<_>>();

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

        let records = output
            .repodata
            .iter()
            .flat_map(|repodata| repodata.iter())
            .map(|record| JsValue::from(JsRepoDataRecord::from(record.clone())))
            .collect::<js_sys::Array>();
        let result = js_sys::Object::new();
        js_sys::Reflect::set(&result, &JsValue::from_str("records"), &records)
            .expect("setting a property on a plain object cannot fail");
        js_sys::Reflect::set(
            &result,
            &JsValue::from_str("warnings"),
            &serde_wasm_bindgen::to_value(&warnings)?,
        )
        .expect("setting a property on a plain object cannot fail");
        Ok(result.into())
    }
}
