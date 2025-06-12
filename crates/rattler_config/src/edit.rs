use std::path::Path;
use std::path::PathBuf;

use miette::Context;
use miette::IntoDiagnostic;
use miette::miette;
use serde::de::DeserializeOwned;
use url::Url;

use crate::config::Config;
use crate::config::ConfigBase;
use crate::config::proxy::ProxyConfig;
use crate::config::s3::S3Options;

impl<T> ConfigBase<T>
where
    T: Config + DeserializeOwned,
{
    /// Modify this config with the given key and value
    ///
    /// # Note
    ///
    /// It is required to call `save()` to persist the changes.
    pub fn set(&mut self, key: &str, value: Option<String>) -> Result<(), miette::Error> {
        let show_supported_keys = || format!("Supported keys:\n\t{}", self.keys().join(",\n\t"));
        // let err = ConfigError::UnknownKey((key.to_string(), show_supported_keys()));
        let err = miette::miette!(
            "Unknown key: {}\n{}",
            console::style(key).red(),
            show_supported_keys()
        );

        match key {
            "default-channels" => {
                self.default_channels = value
                    .map(|v| serde_json::de::from_str(&v))
                    .transpose()
                    .into_diagnostic()?
                    .unwrap_or_default();
            }
            "authentication-override-file" => {
                self.authentication_override_file = value.map(PathBuf::from);
            }
            "tls-no-verify" => {
                self.tls_no_verify = value.map(|v| v.parse()).transpose().into_diagnostic()?;
            }
            "mirrors" => {
                self.mirrors = value
                    .map(|v| serde_json::de::from_str(&v))
                    .transpose()
                    .into_diagnostic()?
                    .unwrap_or_default();
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
                        .map(|v| serde_json::de::from_str(&v))
                        .transpose()
                        .into_diagnostic()?
                        .unwrap_or_default();
                    return Ok(());
                } else if !key.starts_with("repodata-config.") {
                    return Err(err);
                }

                let subkey = key.strip_prefix("repodata-config.").unwrap();
                match subkey {
                    "disable-jlap" => {
                        self.repodata_config.default.disable_jlap =
                            value.map(|v| v.parse()).transpose().into_diagnostic()?;
                    }
                    "disable-bzip2" => {
                        self.repodata_config.default.disable_bzip2 =
                            value.map(|v| v.parse()).transpose().into_diagnostic()?;
                    }
                    "disable-zstd" => {
                        self.repodata_config.default.disable_zstd =
                            value.map(|v| v.parse()).transpose().into_diagnostic()?;
                    }
                    "disable-sharded" => {
                        self.repodata_config.default.disable_sharded =
                            value.map(|v| v.parse()).transpose().into_diagnostic()?;
                    }
                    _ => return Err(err),
                }
            }
            key if key.starts_with("s3-options") => {
                if key == "s3-options" {
                    if let Some(value) = value {
                        self.s3_options = serde_json::de::from_str(&value).into_diagnostic()?;
                    } else {
                        return Err(miette!("s3-options requires a value"));
                    }
                    return Ok(());
                }
                let Some(subkey) = key.strip_prefix("s3-options.") else {
                    return Err(err);
                };
                if let Some((bucket, rest)) = subkey.split_once('.') {
                    if let Some(bucket_config) = self.s3_options.get_mut(bucket) {
                        match rest {
                            "endpoint-url" => {
                                if let Some(value) = value {
                                    bucket_config.endpoint_url =
                                        Url::parse(&value).into_diagnostic()?;
                                } else {
                                    return Err(miette!(
                                        "s3-options.{}.endpoint-url requires a value",
                                        bucket
                                    ));
                                }
                            }
                            "region" => {
                                if let Some(value) = value {
                                    bucket_config.region = value;
                                } else {
                                    return Err(miette!(
                                        "s3-options.{}.region requires a value",
                                        bucket
                                    ));
                                }
                            }
                            "force-path-style" => {
                                if let Some(value) = value {
                                    bucket_config.force_path_style =
                                        value.parse().into_diagnostic()?;
                                } else {
                                    return Err(miette!(
                                        "s3-options.{}.force-path-style requires a value",
                                        bucket
                                    ));
                                }
                            }
                            _ => return Err(err),
                        }
                    }
                } else {
                    let value = value.ok_or_else(|| miette!("s3-options requires a value"))?;
                    let s3_options: S3Options =
                        serde_json::de::from_str(&value).into_diagnostic()?;
                    self.s3_options.insert(subkey.to_string(), s3_options);
                }
            }
            key if key.starts_with("concurrency.") => {
                let subkey = key.strip_prefix("concurrency.").unwrap();
                match subkey {
                    "solves" => {
                        if let Some(value) = value {
                            self.concurrency.solves = value.parse().into_diagnostic()?;
                        } else {
                            return Err(miette!("'solves' requires a number value"));
                        }
                    }
                    "downloads" => {
                        if let Some(value) = value {
                            self.concurrency.downloads = value.parse().into_diagnostic()?;
                        } else {
                            return Err(miette!("'downloads' requires a number value"));
                        }
                    }
                    _ => return Err(err),
                }
            }
            key if key.starts_with("run-post-link-scripts") => {
                if let Some(value) = value {
                    self.run_post_link_scripts = Some(
                        value
                            .parse()
                            .into_diagnostic()
                            .wrap_err("failed to parse run-post-link-scripts")?,
                    );
                }
                return Ok(());
            }
            key if key.starts_with("proxy-config") => {
                if key == "proxy-config" {
                    if let Some(value) = value {
                        self.proxy_config = serde_json::de::from_str(&value).into_diagnostic()?;
                    } else {
                        self.proxy_config = ProxyConfig::default();
                    }
                    return Ok(());
                } else if !key.starts_with("proxy-config.") {
                    return Err(err);
                }

                let subkey = key.strip_prefix("proxy-config.").unwrap();
                match subkey {
                    "https" => {
                        self.proxy_config.https = value
                            .map(|v| Url::parse(&v))
                            .transpose()
                            .into_diagnostic()?;
                    }
                    "http" => {
                        self.proxy_config.http = value
                            .map(|v| Url::parse(&v))
                            .transpose()
                            .into_diagnostic()?;
                    }
                    "non-proxy-hosts" => {
                        self.proxy_config.non_proxy_hosts = value
                            .map(|v| serde_json::de::from_str(&v))
                            .transpose()
                            .into_diagnostic()?
                            .unwrap_or_default();
                    }
                    _ => return Err(err),
                }
            }
            _ => {
                // We don't know this key, but possibly an extension does.
                // self.extensions.set(key, value)
                //     .into_diagnostic()
                //     .wrap_err(format!("failed to set extension key '{}'", key))?;
            }
        }

        Ok(())
    }

    /// Save the config to the given path.
    pub fn save(&self, to: &Path) -> Result<(), miette::Error> {
        let contents = toml::to_string_pretty(&self).into_diagnostic()?;
        tracing::debug!("Saving config to: {}", to.display());

        let parent = to.parent().expect("config path should have a parent");
        fs_err::create_dir_all(parent).into_diagnostic()?;

        fs_err::write(to, contents).into_diagnostic()?;
        Ok(())
    }
}
