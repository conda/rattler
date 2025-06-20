use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{config::Config, edit::ConfigEditError};

#[derive(Default, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct S3OptionsMap(pub IndexMap<String, S3Options>);

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct S3Options {
    /// S3 endpoint URL
    pub endpoint_url: Url,

    /// The name of the S3 region
    pub region: String,

    /// Force path style URLs instead of subdomain style
    pub force_path_style: bool,
}

impl Config for S3OptionsMap {
    fn is_default(&self) -> bool {
        self.0.is_empty()
    }

    fn merge_config(self, other: &Self) -> Result<Self, super::MergeError> {
        // Merge the two S3OptionMaps, overwriting existing keys
        let mut merged = self.0.clone();
        for (key, value) in &other.0 {
            merged.insert(key.clone(), value.clone());
        }
        Ok(S3OptionsMap(merged))
    }

    #[cfg(feature = "edit")]
    fn set(&mut self, key: &str, value: Option<String>) -> Result<(), ConfigEditError> {
        if key == "s3-options" {
            let value = value.ok_or_else(|| ConfigEditError::MissingValue {
                key: key.to_string(),
            })?;
            self.0 =
                serde_json::de::from_str(&value).map_err(|e| ConfigEditError::JsonParseError {
                    key: key.to_string(),
                    source: e,
                })?;
            return Ok(());
        }
        let Some(subkey) = key.strip_prefix("s3-options.") else {
            return Err(ConfigEditError::UnknownKey {
                key: key.to_string(),
                supported_keys: "".to_string(),
            });
        };
        if let Some((bucket, rest)) = subkey.split_once('.') {
            if !self.0.contains_key(bucket) {
                return Err(ConfigEditError::BucketNotFound {
                    bucket: bucket.to_string(),
                });
            }
            let bucket_config = self.0.get_mut(bucket).unwrap();
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
                    bucket_config.region = value.ok_or_else(|| ConfigEditError::MissingValue {
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
                        supported_keys: "".to_string(),
                    })
                }
            }
        } else {
            let value = value.ok_or_else(|| ConfigEditError::MissingValue {
                key: key.to_string(),
            })?;
            let s3_options: S3Options =
                serde_json::de::from_str(&value).map_err(|e| ConfigEditError::JsonParseError {
                    key: key.to_string(),
                    source: e,
                })?;
            self.0.insert(subkey.to_string(), s3_options);
        }
        Ok(())
    }

    fn get_extension_name(&self) -> String {
        "s3-options".to_string()
    }

    fn validate(&self) -> Result<(), super::ValidationError> {
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        self.0.keys().map(|key| key.to_string()).collect()
    }
}
