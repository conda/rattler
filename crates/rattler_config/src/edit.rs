use std::path::Path;
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use thiserror::Error;
use url::Url;

use crate::config::proxy::ProxyConfig;
use crate::config::s3::S3Options;
use crate::config::Config;
use crate::config::ConfigBase;

#[derive(Error, Debug)]
pub enum ConfigEditError {
    #[error("Unknown configuration key: {key}\nSupported keys:\n\t{supported_keys}")]
    UnknownKey { key: String, supported_keys: String },

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
    /// # Note
    ///
    /// It is required to call `save()` to persist the changes.
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
            // "detached-environments" => {
            //     self.detached_environments = value.map(|v| match v.as_str() {
            //         "true" => DetachedEnvironments::Boolean(true),
            //         "false" => DetachedEnvironments::Boolean(false),
            //         _ => DetachedEnvironments::Path(PathBuf::from(v)),
            //     });
            // }
            // "pinning-strategy" => {
            //     self.pinning_strategy = value
            //         .map(|v| PinningStrategy::from_str(v.as_str()))
            //         .transpose()
            //         .into_diagnostic()?
            // }
            // "change-ps1" => {
            //     return Err(miette::miette!(
            //         "The `change-ps1` field is deprecated. Please use the `shell.change-ps1` field instead."
            //     ));
            // }
            // "force-activate" => {
            //     return Err(miette::miette!(
            //         "The `force-activate` field is deprecated. Please use the `shell.force-activate` field instead."
            //     ));
            // }
            key if key.starts_with("repodata-config") => {
                if key == "repodata-config" {
                    self.repodata_config = value
                        .map(|v| {
                            serde_json::de::from_str(&v).map_err(|e| {
                                ConfigEditError::JsonParseError {
                                    key: key.to_string(),
                                    source: e,
                                }
                            })
                        })
                        .transpose()?
                        .unwrap_or_default();
                    return Ok(());
                } else if !key.starts_with("repodata-config.") {
                    return Err(ConfigEditError::UnknownKey {
                        key: key.to_string(),
                        supported_keys: get_supported_keys(self),
                    });
                }

                let subkey = key.strip_prefix("repodata-config.").unwrap();
                match subkey {
                    "disable-jlap" => {
                        self.repodata_config.default.disable_jlap = value
                            .map(|v| {
                                v.parse().map_err(|e| ConfigEditError::BoolParseError {
                                    key: key.to_string(),
                                    source: e,
                                })
                            })
                            .transpose()?;
                    }
                    "disable-bzip2" => {
                        self.repodata_config.default.disable_bzip2 = value
                            .map(|v| {
                                v.parse().map_err(|e| ConfigEditError::BoolParseError {
                                    key: key.to_string(),
                                    source: e,
                                })
                            })
                            .transpose()?;
                    }
                    "disable-zstd" => {
                        self.repodata_config.default.disable_zstd = value
                            .map(|v| {
                                v.parse().map_err(|e| ConfigEditError::BoolParseError {
                                    key: key.to_string(),
                                    source: e,
                                })
                            })
                            .transpose()?;
                    }
                    "disable-sharded" => {
                        self.repodata_config.default.disable_sharded = value
                            .map(|v| {
                                v.parse().map_err(|e| ConfigEditError::BoolParseError {
                                    key: key.to_string(),
                                    source: e,
                                })
                            })
                            .transpose()?;
                    }
                    _ => {
                        return Err(ConfigEditError::UnknownKey {
                            key: key.to_string(),
                            supported_keys: get_supported_keys(self),
                        })
                    }
                }
                Ok(())
            }
            key if key.starts_with("s3-options") => {
                if key == "s3-options" {
                    let value = value.ok_or_else(|| ConfigEditError::MissingValue {
                        key: key.to_string(),
                    })?;
                    self.s3_options = serde_json::de::from_str(&value).map_err(|e| {
                        ConfigEditError::JsonParseError {
                            key: key.to_string(),
                            source: e,
                        }
                    })?;
                    return Ok(());
                }
                let Some(subkey) = key.strip_prefix("s3-options.") else {
                    return Err(ConfigEditError::UnknownKey {
                        key: key.to_string(),
                        supported_keys: get_supported_keys(self),
                    });
                };
                if let Some((bucket, rest)) = subkey.split_once('.') {
                    if !self.s3_options.contains_key(bucket) {
                        return Err(ConfigEditError::BucketNotFound {
                            bucket: bucket.to_string(),
                        });
                    }
                    let bucket_config = self.s3_options.get_mut(bucket).unwrap();
                    match rest {
                        "endpoint-url" => {
                            let value = value.ok_or_else(|| ConfigEditError::MissingValue {
                                key: key.to_string(),
                            })?;
                            bucket_config.endpoint_url =
                                Url::parse(&value).map_err(|e| ConfigEditError::UrlParseError {
                                    key: key.to_string(),
                                    source: e,
                                })?;
                        }
                        "region" => {
                            bucket_config.region =
                                value.ok_or_else(|| ConfigEditError::MissingValue {
                                    key: key.to_string(),
                                })?;
                        }
                        "force-path-style" => {
                            let value = value.ok_or_else(|| ConfigEditError::MissingValue {
                                key: key.to_string(),
                            })?;
                            bucket_config.force_path_style =
                                value.parse().map_err(|e| ConfigEditError::BoolParseError {
                                    key: key.to_string(),
                                    source: e,
                                })?;
                        }
                        _ => {
                            return Err(ConfigEditError::UnknownKey {
                                key: key.to_string(),
                                supported_keys: get_supported_keys(self),
                            })
                        }
                    }
                } else {
                    let value = value.ok_or_else(|| ConfigEditError::MissingValue {
                        key: key.to_string(),
                    })?;
                    let s3_options: S3Options = serde_json::de::from_str(&value).map_err(|e| {
                        ConfigEditError::JsonParseError {
                            key: key.to_string(),
                            source: e,
                        }
                    })?;
                    self.s3_options.insert(subkey.to_string(), s3_options);
                }
                Ok(())
            }
            key if key.starts_with("concurrency.") => {
                let subkey = key.strip_prefix("concurrency.").unwrap();
                match subkey {
                    "solves" => {
                        let value = value.ok_or_else(|| ConfigEditError::MissingValue {
                            key: key.to_string(),
                        })?;
                        self.concurrency.solves =
                            value
                                .parse()
                                .map_err(|e| ConfigEditError::NumberParseError {
                                    key: key.to_string(),
                                    source: e,
                                })?;
                    }
                    "downloads" => {
                        let value = value.ok_or_else(|| ConfigEditError::MissingValue {
                            key: key.to_string(),
                        })?;
                        self.concurrency.downloads =
                            value
                                .parse()
                                .map_err(|e| ConfigEditError::NumberParseError {
                                    key: key.to_string(),
                                    source: e,
                                })?;
                    }
                    _ => {
                        return Err(ConfigEditError::UnknownKey {
                            key: key.to_string(),
                            supported_keys: get_supported_keys(self),
                        })
                    }
                }
                Ok(())
            }
            key if key.starts_with("run-post-link-scripts") => {
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
            key if key.starts_with("proxy-config") => {
                if key == "proxy-config" {
                    if let Some(value) = value {
                        self.proxy_config = serde_json::de::from_str(&value).map_err(|e| {
                            ConfigEditError::JsonParseError {
                                key: key.to_string(),
                                source: e,
                            }
                        })?;
                    } else {
                        self.proxy_config = ProxyConfig::default();
                    }
                    return Ok(());
                } else if !key.starts_with("proxy-config.") {
                    return Err(ConfigEditError::UnknownKey {
                        key: key.to_string(),
                        supported_keys: get_supported_keys(self),
                    });
                }

                let subkey = key.strip_prefix("proxy-config.").unwrap();
                match subkey {
                    "https" => {
                        self.proxy_config.https = value
                            .map(|v| {
                                Url::parse(&v).map_err(|e| ConfigEditError::UrlParseError {
                                    key: key.to_string(),
                                    source: e,
                                })
                            })
                            .transpose()?;
                    }
                    "http" => {
                        self.proxy_config.http = value
                            .map(|v| {
                                Url::parse(&v).map_err(|e| ConfigEditError::UrlParseError {
                                    key: key.to_string(),
                                    source: e,
                                })
                            })
                            .transpose()?;
                    }
                    "non-proxy-hosts" => {
                        self.proxy_config.non_proxy_hosts = value
                            .map(|v| {
                                serde_json::de::from_str(&v).map_err(|e| {
                                    ConfigEditError::JsonParseError {
                                        key: key.to_string(),
                                        source: e,
                                    }
                                })
                            })
                            .transpose()?
                            .unwrap_or_default();
                    }
                    _ => {
                        return Err(ConfigEditError::UnknownKey {
                            key: key.to_string(),
                            supported_keys: get_supported_keys(self),
                        })
                    }
                }
                Ok(())
            }
            _ => {
                // We don't know this key, but possibly an extension does.
                // self.extensions.set(key, value)
                //     .into_diagnostic()
                //     .wrap_err(format!("failed to set extension key '{}'", key))?;
                Err(ConfigEditError::UnknownKey {
                    key: key.to_string(),
                    supported_keys: get_supported_keys(self),
                })
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
