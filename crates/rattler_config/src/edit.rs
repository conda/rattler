use std::path::{Path, PathBuf};

use crate::config::{Config, ConfigBase};
use serde::de::DeserializeOwned;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigEditError {
    #[error("Unknown configuration key: {key}\nSupported keys:\n\t{supported_keys}")]
    UnknownKey { key: String, supported_keys: String },

    #[error("Unknown key: {key}")]
    UnknownKeyInner { key: String },

    #[error("Configuration key '{key}' requires a value")]
    MissingValue { key: String },

    #[error("Invalid value for '{key}': {source}")]
    InvalidValue {
        key: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("Failed to parse JSON for '{key}': {source}")]
    JsonParseError {
        key: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("Failed to parse URL for '{key}': {source}")]
    UrlParseError {
        key: String,
        #[source]
        source: url::ParseError,
    },

    #[error("Failed to parse boolean for '{key}': {source}")]
    BoolParseError {
        key: String,
        #[source]
        source: std::str::ParseBoolError,
    },

    #[error("Failed to parse number for '{key}': {source}")]
    NumberParseError {
        key: String,
        #[source]
        source: std::num::ParseIntError,
    },

    #[error("Bucket '{bucket}' not found in s3-options")]
    BucketNotFound { bucket: String },

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("TOML serialization error: {0}")]
    TomlSerializeError(#[from] toml::ser::Error),
}

impl<T> ConfigBase<T>
where
    T: Config + DeserializeOwned,
{
    /// Modify this config with the given key and value
    ///
    /// It is required to call `save()` to persist the changes on disk.
    pub fn set(&mut self, key: &str, value: Option<String>) -> Result<(), ConfigEditError> {
        let get_supported_keys = |config: &Self| config.keys().join(",\n\t");

        match key {
            "default-channels" => {
                self.default_channels = value
                    .map(|v| {
                        serde_json::de::from_str(&v).map_err(|e| ConfigEditError::JsonParseError {
                            key: key.to_string(),
                            source: e,
                        })
                    })
                    .transpose()?
                    .unwrap_or_default();
                Ok(())
            }
            "authentication-override-file" => {
                self.authentication_override_file = value.map(PathBuf::from);
                Ok(())
            }
            "tls-no-verify" => {
                self.tls_no_verify = value
                    .map(|v| {
                        v.parse().map_err(|e| ConfigEditError::BoolParseError {
                            key: key.to_string(),
                            source: e,
                        })
                    })
                    .transpose()?;
                Ok(())
            }
            "mirrors" => {
                self.mirrors = value
                    .map(|v| {
                        serde_json::de::from_str(&v).map_err(|e| ConfigEditError::JsonParseError {
                            key: key.to_string(),
                            source: e,
                        })
                    })
                    .transpose()?
                    .unwrap_or_default();
                Ok(())
            }
            "run-post-link-scripts" => {
                let value = value.ok_or_else(|| ConfigEditError::MissingValue {
                    key: key.to_string(),
                })?;
                self.run_post_link_scripts =
                    Some(value.parse().map_err(|e| ConfigEditError::InvalidValue {
                        key: key.to_string(),
                        source: Box::new(e),
                    })?);
                Ok(())
            }
            key if key.starts_with("repodata-config") => {
                self.repodata_config.set(key, value)?;
                Ok(())
            }
            key if key.starts_with("s3-options") => {
                self.s3_options.set(key, value)?;
                Ok(())
            }
            key if key.starts_with("concurrency.") => {
                self.concurrency.set(key, value)?;
                Ok(())
            }
            key if key.starts_with("proxy-config") => {
                self.proxy_config.set(key, value)?;
                Ok(())
            }
            _ => {
                // We don't know this key, but possibly an extension does.
                self.extensions.set(key, value).map_err(|e| match e {
                    // Update the error to include all supported keys.
                    ConfigEditError::UnknownKey {
                        key,
                        supported_keys: _,
                    } => ConfigEditError::UnknownKey {
                        key,
                        supported_keys: get_supported_keys(self),
                    },
                    _ => e,
                })?;
                Ok(())
            }
        }
    }

    /// Save the config to the given path.
    pub fn save(&self, to: &Path) -> Result<(), ConfigEditError> {
        let contents = self.to_toml()?;
        tracing::debug!("Saving config to: {}", to.display());

        let parent = to.parent().expect("config path should have a parent");
        fs_err::create_dir_all(parent)?;

        fs_err::write(to, contents)?;
        Ok(())
    }

    /// Convert the config to a TOML string.
    pub fn to_toml(&self) -> Result<String, ConfigEditError> {
        toml::to_string_pretty(&self).map_err(ConfigEditError::TomlSerializeError)
    }
}
