use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::Config;
#[cfg(feature = "edit")]
use crate::edit::ConfigEditError;

#[derive(Default, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AzureOptionsMap(pub IndexMap<String, AzureOptions>);

impl AzureOptionsMap {
    /// Returns `true` if no Azure containers are configured.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct AzureOptions {
    /// Storage account name → host `{account}.blob.core.windows.net`.
    pub account: String,

    /// Optional full endpoint override for sovereign clouds / Azurite.
    /// Defaults to `https://{account}.blob.core.windows.net`.
    pub endpoint_url: Option<Url>,
}

impl Config for AzureOptionsMap {
    fn is_default(&self) -> bool {
        self.0.is_empty()
    }

    fn merge_config(self, other: &Self) -> Result<Self, super::MergeError> {
        let mut merged = self.0.clone();
        for (key, value) in &other.0 {
            merged.insert(key.clone(), value.clone());
        }
        Ok(AzureOptionsMap(merged))
    }

    #[cfg(feature = "edit")]
    fn set(&mut self, key: &str, value: Option<String>) -> Result<(), ConfigEditError> {
        if key == "azure-options" {
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
        let Some(subkey) = key.strip_prefix("azure-options.") else {
            return Err(ConfigEditError::UnknownKey {
                key: key.to_string(),
                supported_keys: "".to_string(),
            });
        };
        if let Some((container, rest)) = subkey.split_once('.') {
            if !self.0.contains_key(container) {
                return Err(ConfigEditError::BucketNotFound {
                    bucket: container.to_string(),
                });
            }
            let container_config = self.0.get_mut(container).unwrap();
            match rest {
                "account" => {
                    container_config.account =
                        value.ok_or_else(|| ConfigEditError::MissingValue {
                            key: key.to_string(),
                        })?;
                }
                "endpoint-url" => {
                    let value = value.ok_or_else(|| ConfigEditError::MissingValue {
                        key: key.to_string(),
                    })?;
                    container_config.endpoint_url = Some(Url::parse(&value).map_err(|e| {
                        ConfigEditError::UrlParseError {
                            key: key.to_string(),
                            source: e,
                        }
                    })?);
                }
                _ => {
                    return Err(ConfigEditError::UnknownKey {
                        key: key.to_string(),
                        supported_keys: "".to_string(),
                    });
                }
            }
        } else {
            let value = value.ok_or_else(|| ConfigEditError::MissingValue {
                key: key.to_string(),
            })?;
            let azure_options: AzureOptions =
                serde_json::de::from_str(&value).map_err(|e| ConfigEditError::JsonParseError {
                    key: key.to_string(),
                    source: e,
                })?;
            self.0.insert(subkey.to_string(), azure_options);
        }
        Ok(())
    }

    fn get_extension_name(&self) -> String {
        "azure-options".to_string()
    }

    fn validate(&self) -> Result<(), super::ValidationError> {
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        self.0.keys().map(ToString::to_string).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_account_and_optional_endpoint() {
        let toml = r#"
            [mychannel]
            account = "myacct"

            [other]
            account = "acct2"
            endpoint-url = "https://acct2.blob.core.windows.net"
        "#;
        let map: AzureOptionsMap = toml::from_str(toml).unwrap();
        let mychannel = map.0.get("mychannel").unwrap();
        assert_eq!(mychannel.account, "myacct");
        assert_eq!(mychannel.endpoint_url, None);
        let other = map.0.get("other").unwrap();
        assert_eq!(
            other.endpoint_url.as_ref().unwrap().as_str(),
            "https://acct2.blob.core.windows.net/"
        );
    }

    #[test]
    fn is_default_when_empty() {
        let map = AzureOptionsMap::default();
        assert!(map.is_default());
    }

    #[test]
    fn merge_overwrites_existing_keys() {
        let mut base = AzureOptionsMap::default();
        base.0.insert(
            "c".to_string(),
            AzureOptions { account: "old".into(), endpoint_url: None },
        );
        let mut other = AzureOptionsMap::default();
        other.0.insert(
            "c".to_string(),
            AzureOptions { account: "new".into(), endpoint_url: None },
        );
        let merged = base.merge_config(&other).unwrap();
        assert_eq!(merged.0.get("c").unwrap().account, "new");
    }
}
