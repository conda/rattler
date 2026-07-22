//! The shared configuration model for rattler-based tools.
//!
//! The entry point is [`ConfigBase<T>`]: the set of configuration keys that
//! every rattler-based tool (pixi, rattler-build, rattler-index, …)
//! understands, plus a tool-specific *extension* `T` whose keys live at the
//! top level of the same TOML document.
//!
//! See the crate-level documentation for a full example of writing an
//! extension.

use std::{
    collections::BTreeSet,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
};

use indexmap::IndexMap;
use rattler_conda_types::{ChannelConfig, NamedChannelOrUrl};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use url::Url;

use crate::config::s3::S3OptionsMap;
use crate::config::{
    build::BuildConfig, concurrency::ConcurrencyConfig, index::IndexConfig, proxy::ProxyConfig,
    repodata_config::RepodataConfig, run_post_link_scripts::RunPostLinkScripts,
};

pub mod build;
pub mod channel_config;
pub mod concurrency;
pub mod index;
pub mod proxy;
pub mod repodata_config;
pub mod run_post_link_scripts;
pub mod s3;
pub mod tls;
use crate::config::channel_config::default_channel_config;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    /// Missing required field.
    #[error("Missing required field: {0}")]
    MissingRequiredField(String),

    /// Invalid value for a field.
    #[error("Invalid value for field {0}: {1}")]
    InvalidValue(String, String),

    /// Invalid configuration for various reason.
    #[error("Invalid configuration: {0}")]
    Invalid(String),
}

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum MergeError {
    /// Error merging configurations.
    #[error("Error merging configurations: {0}")]
    Error(String),
}

#[derive(Error, Debug)]
pub enum LoadError {
    /// Error loading configuration.
    #[error("Error merging configuration files: {0} ({1})")]
    MergeError(MergeError, PathBuf),

    /// IO error while reading configuration file.
    #[error("IO error while reading configuration file: {0}")]
    IoError(#[from] std::io::Error),

    /// Error parsing configuration file.
    #[error("Error parsing configuration file: {0}")]
    ParseError(#[from] toml::de::Error),

    /// Error validating configuration.
    #[error("Error validating configuration: {0}")]
    ValidationError(#[from] ValidationError),
}

/// A fragment of configuration: either a nested section (like
/// [`ConcurrencyConfig`]) or a tool-specific extension plugged into
/// [`ConfigBase`].
///
/// Implementors only *have* to provide [`Config::merge_config`]; everything
/// else has a sensible default. Extensions must **not** use
/// `#[serde(deny_unknown_fields)]`: an extension is deserialized from the
/// full configuration document, so it has to tolerate the keys handled by
/// [`CommonConfig`] (and report them as ignored, which the derive does by
/// default).
pub trait Config:
    Serialize + DeserializeOwned + std::fmt::Debug + Clone + PartialEq + Default
{
    /// Merge another configuration (file) into this one.
    /// Note: the "other" configuration takes priority over the current one.
    fn merge_config(self, other: &Self) -> Result<Self, MergeError>;

    /// Validate the configuration. Called after all configuration files have
    /// been merged.
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }

    /// Returns true if the configuration equals its default value.
    fn is_default(&self) -> bool {
        self == &Self::default()
    }

    /// The dotted TOML key paths understood by this fragment, used to build
    /// "supported keys" listings for error messages of `set`.
    ///
    /// Keys are relative to the fragment itself; [`ConfigBase`] adds the
    /// proper prefixes for nested sections.
    fn keys(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Extension type for tools that have no tool-specific configuration keys.
///
/// This is the default extension of [`ConfigBase`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoExtension {}

impl Config for NoExtension {
    fn merge_config(self, _other: &Self) -> Result<Self, MergeError> {
        Ok(self)
    }
}

/// The configuration keys shared by all rattler-based tools.
///
/// This struct deliberately contains no `#[serde(flatten)]` fields: unknown
/// top-level keys must surface through `serde_ignored` so that
/// [`ConfigBase::from_toml_str`] can report typos and tool-specific keys
/// reliably (`flatten` swallows unknown keys before `serde_ignored` can see
/// them).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CommonConfig {
    #[serde(default)]
    #[serde(alias = "default_channels")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_channels: Option<Vec<NamedChannelOrUrl>>,

    /// Path to the file containing the authentication token.
    #[serde(default)]
    #[serde(alias = "authentication_override_file")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication_override_file: Option<PathBuf>,

    /// If set to true, the HTTPS client will not verify TLS server
    /// certificates.
    #[serde(default)]
    #[serde(alias = "tls_no_verify")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_no_verify: Option<bool>,

    /// Which TLS root certificates to use (webpki vs system).
    /// Accepts legacy spellings `"native"` and `"all"` as aliases for
    /// `"system"`. See [`tls::TlsRootCerts`] for details.
    #[serde(default)]
    #[serde(alias = "tls_root_certs")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_root_certs: Option<tls::TlsRootCerts>,

    #[serde(default)]
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub mirrors: IndexMap<Url, Vec<Url>>,

    #[serde(default, skip_serializing_if = "BuildConfig::is_default")]
    pub build: BuildConfig,

    #[serde(skip, default = "default_channel_config")]
    pub channel_config: ChannelConfig,

    /// Configuration for repodata fetching.
    #[serde(alias = "repodata_config")] // BREAK: remove to stop supporting snake_case alias
    #[serde(default, skip_serializing_if = "RepodataConfig::is_empty")]
    pub repodata_config: RepodataConfig,

    /// Configuration for the concurrency of rattler.
    #[serde(default)]
    #[serde(skip_serializing_if = "ConcurrencyConfig::is_default")]
    pub concurrency: ConcurrencyConfig,

    /// Https/Http proxy configuration.
    #[serde(default)]
    #[serde(skip_serializing_if = "ProxyConfig::is_default")]
    pub proxy_config: ProxyConfig,

    /// Configuration for S3.
    #[serde(default)]
    #[serde(skip_serializing_if = "S3OptionsMap::is_default")]
    pub s3_options: S3OptionsMap,

    /// Per-channel configuration for `rattler-index`.
    #[serde(default, skip_serializing_if = "IndexConfig::is_empty")]
    pub index_config: IndexConfig,

    /// Run the post link scripts
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_post_link_scripts: Option<RunPostLinkScripts>,

    /// If set to false, symbolic links will not be used during package
    /// installation.
    #[serde(default)]
    #[serde(alias = "allow_symbolic_links")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_symbolic_links: Option<bool>,

    /// If set to false, hard links will not be used during package
    /// installation.
    #[serde(default)]
    #[serde(alias = "allow_hard_links")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_hard_links: Option<bool>,

    /// If set to false, ref links (copy-on-write) will not be used during
    /// package installation.
    #[serde(default)]
    #[serde(alias = "allow_ref_links")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_ref_links: Option<bool>,
    // Missing in rattler but should be available in pixi:
    //   experimental
    //   shell
    //   pinning_strategy
    //   detached_environments
    //   pypi_config
    //
    // Deprecated fields:
    //   change_ps1
    //   force_activate
}

// ChannelConfig does not implement `Default` so we need to provide a default
// implementation.
impl Default for CommonConfig {
    fn default() -> Self {
        Self {
            default_channels: None,
            authentication_override_file: None,
            tls_no_verify: None,
            tls_root_certs: None,
            mirrors: IndexMap::new(),
            build: BuildConfig::default(),
            channel_config: default_channel_config(),
            repodata_config: RepodataConfig::default(),
            concurrency: ConcurrencyConfig::default(),
            proxy_config: ProxyConfig::default(),
            s3_options: S3OptionsMap::default(),
            index_config: IndexConfig::default(),
            run_post_link_scripts: None,
            allow_symbolic_links: None,
            allow_hard_links: None,
            allow_ref_links: None,
        }
    }
}

/// Prefix every key in `keys` with `prefix.`.
fn prefixed_keys(prefix: &str, keys: Vec<String>) -> Vec<String> {
    keys.into_iter().map(|k| format!("{prefix}.{k}")).collect()
}

/// Is `key` equal to, or nested inside, one of the `ignored` paths?
fn covered_by(key: &str, ignored: &BTreeSet<String>) -> bool {
    ignored.iter().any(|path| {
        key == path
            || key
                .strip_prefix(path.as_str())
                .is_some_and(|rest| rest.starts_with('.'))
    })
}

impl Config for CommonConfig {
    /// Merge another configuration (file) into this one.
    /// Note: the "other" configuration takes priority over the current one.
    fn merge_config(self, other: &Self) -> Result<Self, MergeError> {
        Ok(Self {
            default_channels: other
                .default_channels
                .as_ref()
                .or(self.default_channels.as_ref())
                .cloned(),
            authentication_override_file: other
                .authentication_override_file
                .as_ref()
                .or(self.authentication_override_file.as_ref())
                .cloned(),
            tls_no_verify: other.tls_no_verify.or(self.tls_no_verify),
            tls_root_certs: other.tls_root_certs.or(self.tls_root_certs),
            mirrors: self
                .mirrors
                .iter()
                .chain(other.mirrors.iter())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            build: self.build.merge_config(&other.build)?,
            // Currently this is always the default so it doesn't matter which one we take.
            channel_config: self.channel_config,
            repodata_config: self.repodata_config.merge_config(&other.repodata_config)?,
            concurrency: self.concurrency.merge_config(&other.concurrency)?,
            proxy_config: self.proxy_config.merge_config(&other.proxy_config)?,
            s3_options: self.s3_options.merge_config(&other.s3_options)?,
            index_config: self.index_config.merge_config(&other.index_config)?,
            run_post_link_scripts: other
                .run_post_link_scripts
                .clone()
                .or(self.run_post_link_scripts),
            allow_symbolic_links: other.allow_symbolic_links.or(self.allow_symbolic_links),
            allow_hard_links: other.allow_hard_links.or(self.allow_hard_links),
            allow_ref_links: other.allow_ref_links.or(self.allow_ref_links),
        })
    }

    fn validate(&self) -> Result<(), ValidationError> {
        self.build.validate()?;
        self.repodata_config.validate()?;
        self.concurrency.validate()?;
        self.proxy_config.validate()?;
        self.s3_options.validate()?;
        self.index_config.validate()?;
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        let mut keys = vec![
            "default-channels".to_string(),
            "authentication-override-file".to_string(),
            "tls-no-verify".to_string(),
            "tls-root-certs".to_string(),
            "mirrors".to_string(),
            "run-post-link-scripts".to_string(),
            "allow-symbolic-links".to_string(),
            "allow-hard-links".to_string(),
            "allow-ref-links".to_string(),
            "s3-options".to_string(),
            "index-config".to_string(),
        ];
        keys.extend(prefixed_keys("build", self.build.keys()));
        keys.extend(prefixed_keys(
            "repodata-config",
            self.repodata_config.keys(),
        ));
        keys.extend(prefixed_keys("concurrency", self.concurrency.keys()));
        keys.extend(prefixed_keys("proxy-config", self.proxy_config.keys()));
        keys.extend(prefixed_keys("s3-options", self.s3_options.keys()));
        keys
    }
}

/// The complete configuration of a rattler-based tool: the
/// [common keys](CommonConfig) plus a tool-specific extension `T` whose keys
/// live at the top level of the same TOML document.
///
/// `ConfigBase` dereferences to [`CommonConfig`], so the shared keys can be
/// accessed directly: `config.default_channels`, `config.concurrency`, ….
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigBase<T = NoExtension> {
    /// The configuration keys shared by all rattler-based tools.
    #[serde(flatten)]
    pub common: CommonConfig,

    /// Tool-specific configuration keys.
    #[serde(flatten)]
    pub extensions: T,

    /// The configuration files this configuration was loaded from, in load
    /// order (lowest precedence first).
    #[serde(skip)]
    pub loaded_from: Vec<PathBuf>,
}

impl<T> Deref for ConfigBase<T> {
    type Target = CommonConfig;

    fn deref(&self) -> &Self::Target {
        &self.common
    }
}

impl<T> DerefMut for ConfigBase<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.common
    }
}

impl<T> Config for ConfigBase<T>
where
    T: Config,
{
    /// Merge another configuration (file) into this one.
    /// Note: the "other" configuration takes priority over the current one.
    fn merge_config(self, other: &Self) -> Result<Self, MergeError> {
        Ok(Self {
            common: self.common.merge_config(&other.common)?,
            extensions: self.extensions.merge_config(&other.extensions)?,
            loaded_from: self
                .loaded_from
                .iter()
                .chain(&other.loaded_from)
                .cloned()
                .collect(),
        })
    }

    fn validate(&self) -> Result<(), ValidationError> {
        self.common.validate()?;
        self.extensions.validate()
    }

    /// Gather all the keys of the configuration, including the extension
    /// keys.
    fn keys(&self) -> Vec<String> {
        let mut keys = self.common.keys();
        keys.extend(self.extensions.keys());
        keys
    }
}

impl<T> ConfigBase<T>
where
    T: Config,
{
    /// Parse a configuration from a TOML string.
    ///
    /// Returns the parsed configuration together with the set of keys that
    /// neither [`CommonConfig`] nor the extension `T` recognized. Callers
    /// should surface these to the user as warnings (they are typos or
    /// keys of other tools).
    pub fn from_toml_str(input: &str) -> Result<(Self, BTreeSet<String>), toml::de::Error> {
        // The document is deserialized twice: once into the common
        // configuration and once into the extension. Each pass records the
        // keys it did not recognize; only keys unknown to *both* passes are
        // truly unused. This is also why `CommonConfig` must not contain
        // `#[serde(flatten)]` fields: flattening swallows unknown keys
        // before `serde_ignored` can see them.
        let mut unknown_to_common = BTreeSet::new();
        let common: CommonConfig = serde_ignored::deserialize(
            toml::de::Deserializer::parse(input)?,
            |path: serde_ignored::Path<'_>| {
                unknown_to_common.insert(path.to_string());
            },
        )?;

        let mut unknown_to_extension = BTreeSet::new();
        let extensions: T = serde_ignored::deserialize(
            toml::de::Deserializer::parse(input)?,
            |path: serde_ignored::Path<'_>| {
                unknown_to_extension.insert(path.to_string());
            },
        )?;

        // A key is truly unused when neither pass consumed it. The two
        // passes may record it at different depths (the extension ignores
        // an entire table that the common configuration partially
        // consumed, or vice versa), so match against ancestors too and
        // keep the most specific path.
        let unused = unknown_to_common
            .iter()
            .filter(|key| covered_by(key, &unknown_to_extension))
            .chain(
                unknown_to_extension
                    .iter()
                    .filter(|key| covered_by(key, &unknown_to_common)),
            )
            .cloned()
            .collect::<BTreeSet<String>>();
        // Drop entries whose descendants are also reported: the deeper
        // path is the actionable one.
        let unused = unused
            .iter()
            .filter(|key| {
                !unused.iter().any(|other| {
                    other
                        .strip_prefix(key.as_str())
                        .is_some_and(|rest| rest.starts_with('.'))
                })
            })
            .cloned()
            .collect();

        Ok((
            Self {
                common,
                extensions,
                loaded_from: Vec::new(),
            },
            unused,
        ))
    }

    /// Load the configuration by merging all the given files, in order:
    /// later files take precedence over earlier ones. Unrecognized keys are
    /// reported as `tracing` warnings; the merged configuration is validated
    /// before it is returned.
    ///
    /// Missing files result in an error; callers that search default
    /// locations should filter for existing files first (see
    /// [`ConfigBase::load_from_default_locations`]).
    pub fn load_from_files<I, P>(paths: I) -> Result<Self, LoadError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut config = Self::default();

        for path in paths {
            let path = path.as_ref();
            let content = fs_err::read_to_string(path)?;
            let (mut other, unused) = Self::from_toml_str(&content)?;
            for key in &unused {
                tracing::warn!(
                    "Ignoring unknown configuration key `{key}` in {}",
                    path.display()
                );
            }
            other.loaded_from.push(path.to_path_buf());
            config = config
                .merge_config(&other)
                .map_err(|e| LoadError::MergeError(e, path.to_path_buf()))?;
        }

        config.validate()?;
        Ok(config)
    }

    /// Load the configuration from the default locations of the given tools
    /// (e.g. `&["pixi", "rattler-build"]`), skipping files that do not
    /// exist. See [`crate::locations::config_search_paths`] for the exact
    /// search order.
    pub fn load_from_default_locations(tool_dirs: &[&str]) -> Result<Self, LoadError> {
        Self::load_from_files(
            crate::locations::config_search_paths(tool_dirs)
                .into_iter()
                .filter(|path| path.is_file()),
        )
    }
}
