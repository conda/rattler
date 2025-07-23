use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use rattler_conda_types::{ChannelConfig, NamedChannelOrUrl};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::config::s3::S3OptionsMap;
use crate::config::{
    build::BuildConfig, concurrency::ConcurrencyConfig, proxy::ProxyConfig,
    repodata_config::RepodataConfig, run_post_link_scripts::RunPostLinkScripts,
};

pub mod build;
pub mod channel_config;
pub mod concurrency;
pub mod proxy;
pub mod repodata_config;
pub mod run_post_link_scripts;
pub mod s3;
use crate::config::channel_config::default_channel_config;
#[cfg(feature = "edit")]
use crate::edit::ConfigEditError;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigBase<T> {
    #[serde(default)]
    #[serde(alias = "default_channels")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_channels: Option<Vec<NamedChannelOrUrl>>,

    /// Path to the file containing the authentication token.
    #[serde(default)]
    #[serde(alias = "authentication_override_file")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication_override_file: Option<PathBuf>,

    /// If set to true, pixi will not verify the TLS certificate of the server.
    #[serde(default)]
    #[serde(alias = "tls_no_verify")] // BREAK: remove to stop supporting snake_case alias
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_no_verify: Option<bool>,

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

    /// Https/Http proxy configuration for pixi
    #[serde(default)]
    #[serde(skip_serializing_if = "ProxyConfig::is_default")]
    pub proxy_config: ProxyConfig,

    /// Configuration for S3.
    #[serde(default)]
    #[serde(skip_serializing_if = "S3OptionsMap::is_default")]
    pub s3_options: S3OptionsMap,

    /// Run the post link scripts
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_post_link_scripts: Option<RunPostLinkScripts>,

    #[serde(flatten)]
    pub extensions: T,

    #[serde(skip)]
    pub loaded_from: Vec<PathBuf>,
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

// ChannelConfig does not implement `Default` so we need to provide a default implementation.
impl<T> Default for ConfigBase<T>
where
    T: Config + DeserializeOwned,
{
    fn default() -> Self {
        Self {
            default_channels: None,
            authentication_override_file: None,
            tls_no_verify: Some(false), // Default to false if not set
            mirrors: IndexMap::new(),
            build: BuildConfig::default(),
            channel_config: default_channel_config(),
            repodata_config: RepodataConfig::default(),
            concurrency: ConcurrencyConfig::default(),
            proxy_config: ProxyConfig::default(),
            s3_options: S3OptionsMap::default(),
            run_post_link_scripts: None,
            extensions: T::default(),
            loaded_from: Vec::new(),
        }
    }
}

/// An empty dummy configuration extension that we can use when no extension is needed.
impl Config for () {
    fn get_extension_name(&self) -> String {
        "__NONE__".to_string()
    }

    fn merge_config(self, _other: &Self) -> Result<Self, MergeError> {
        Ok(())
    }

    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        vec![]
    }
}

pub trait Config:
    Serialize + for<'de> Deserialize<'de> + std::fmt::Debug + Clone + PartialEq + Eq + Default
{
    /// Get the name of the extension.
    fn get_extension_name(&self) -> String;

    /// Merge another configuration (file) into this one.
    /// Note: the "other" configuration should take priority over the current one.
    fn merge_config(self, other: &Self) -> Result<Self, MergeError>;

    /// Validate the configuration.
    fn validate(&self) -> Result<(), ValidationError>;

    fn is_default(&self) -> bool {
        self == &Self::default()
    }

    /// Get the valid keys of the configuration.
    fn keys(&self) -> Vec<String>;

    /// Set a key in the configuration.
    #[cfg(feature = "edit")]
    fn set(&mut self, key: &str, _value: Option<String>) -> Result<(), ConfigEditError> {
        Err(ConfigEditError::UnknownKey {
            key: key.to_string(),
            supported_keys: self.keys().join(", "),
        })
    }
}

impl<T> ConfigBase<T>
where
    T: Config + DeserializeOwned,
{
    pub fn load_from_files<I, P>(paths: I) -> Result<Self, LoadError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut config = ConfigBase::<T>::default();

        for path in paths {
            let content = std::fs::read_to_string(path.as_ref())?;
            let other: ConfigBase<T> = toml::from_str(&content)?;
            config = config
                .merge_config(&other)
                .map_err(|e| LoadError::MergeError(e, path.as_ref().to_path_buf()))?;
        }

        config.validate()?;
        Ok(config)
    }
}

impl<T> Config for ConfigBase<T>
where
    T: Config + Default,
{
    fn get_extension_name(&self) -> String {
        "base".to_string()
    }

    /// Merge another configuration (file) into this one.
    /// Note: the "other" configuration should take priority over the current one.
    fn merge_config(self, other: &Self) -> Result<Self, MergeError> {
        Ok(Self {
            s3_options: self.s3_options.merge_config(&other.s3_options)?,
            // Use the other configuration's default channels if available
            default_channels: other
                .default_channels
                .as_ref()
                .or(self.default_channels.as_ref())
                .cloned(),
            // Currently this is always the default so it doesn't matter which one we take.
            channel_config: self.channel_config,
            authentication_override_file: other
                .authentication_override_file
                .as_ref()
                .or(self.authentication_override_file.as_ref())
                .cloned(),
            tls_no_verify: other.tls_no_verify.or(self.tls_no_verify).or(Some(false)), // Default to false if not set
            mirrors: self
                .mirrors
                .iter()
                .chain(other.mirrors.iter())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            build: self.build.merge_config(&other.build)?,
            repodata_config: self.repodata_config.merge_config(&other.repodata_config)?,
            concurrency: self.concurrency.merge_config(&other.concurrency)?,
            proxy_config: self.proxy_config.merge_config(&other.proxy_config)?,
            extensions: self.extensions.merge_config(&other.extensions)?,
            run_post_link_scripts: other
                .run_post_link_scripts
                .clone()
                .or(self.run_post_link_scripts),
            loaded_from: self
                .loaded_from
                .iter()
                .chain(&other.loaded_from)
                .cloned()
                .collect(),
        })
    }

    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }

    /// Gather all the keys of the configuration.
    fn keys(&self) -> Vec<String> {
        fn get_keys(config: &impl Config) -> Vec<String> {
            config
                .keys()
                .iter()
                .map(|s| format!("{}.{}", config.get_extension_name(), s))
                .collect()
        }

        let mut keys = Vec::new();

        keys.extend(get_keys(&self.build));
        keys.extend(get_keys(&self.repodata_config));
        keys.extend(get_keys(&self.concurrency));
        keys.extend(get_keys(&self.proxy_config));
        keys.extend(get_keys(&self.extensions));
        keys.extend(get_keys(&self.s3_options));

        keys.push("default_channels".to_string());
        keys.push("authentication_override_file".to_string());
        keys.push("tls_no_verify".to_string());
        keys.push("mirrors".to_string());
        keys.push("loaded_from".to_string());
        keys.push("extensions".to_string());
        keys.push("default".to_string());

        keys
    }
}

pub fn load_config<T: for<'de> Deserialize<'de>>(
    config_file: &str,
) -> Result<ConfigBase<T>, Box<dyn std::error::Error>> {
    let config_content = std::fs::read_to_string(config_file)?;
    let config: ConfigBase<T> = toml::from_str(&config_content)?;
    Ok(config)
}
