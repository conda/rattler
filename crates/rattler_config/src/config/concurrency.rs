use serde::{Deserialize, Serialize};

use crate::config::{Config, MergeError, ValidationError};

// Making the default values part of pixi_config to allow for printing the
// default settings in the future.
/// The default maximum number of concurrent solves that can be run at once.
/// Defaulting to the number of CPUs available.
pub fn default_max_concurrent_solves() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

/// The default maximum number of concurrent downloads that can be run at once.
/// 50 is a reasonable default for the number of concurrent downloads.
/// More verification is needed to determine the optimal number.
pub fn default_max_concurrent_downloads() -> usize {
    50
}

/// The default number of bytes requested per range of a large package, as in
/// the AWS `s3transfer` library.
pub fn default_download_part_size() -> u64 {
    8 * 1024 * 1024
}

/// The default number of ranges downloaded at once across all packages, the
/// same as [`default_max_concurrent_downloads`], so as many requests are in
/// flight as before ranged downloads.
pub fn default_download_parts() -> usize {
    default_max_concurrent_downloads()
}

// Saved configs only list these keys when they differ from the default.
fn is_default_download_part_size(value: &u64) -> bool {
    *value == default_download_part_size()
}

fn is_default_download_parts(value: &usize) -> bool {
    *value == default_download_parts()
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct ConcurrencyConfig {
    /// The maximum number of concurrent solves that can be run at once.
    // Needing to set this default next to the default of the full struct to avoid serde defaulting
    // to 0 of partial struct was omitted.
    #[serde(default = "default_max_concurrent_solves")]
    pub solves: usize,

    /// The maximum number of concurrent HTTP requests to make.
    // Needing to set this default next to the default of the full struct to avoid serde defaulting
    // to 0 of partial struct was omitted.
    #[serde(default = "default_max_concurrent_downloads")]
    pub downloads: usize,

    /// The number of bytes requested per range when a large package is
    /// downloaded in parallel ranges.
    #[serde(
        default = "default_download_part_size",
        skip_serializing_if = "is_default_download_part_size"
    )]
    pub download_part_size: u64,

    /// The maximum number of ranges downloaded at once across all packages.
    #[serde(
        default = "default_download_parts",
        skip_serializing_if = "is_default_download_parts"
    )]
    pub download_parts: usize,
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        Self {
            solves: default_max_concurrent_solves(),
            downloads: default_max_concurrent_downloads(),
            download_part_size: default_download_part_size(),
            download_parts: default_download_parts(),
        }
    }
}

impl ConcurrencyConfig {
    pub fn is_default(&self) -> bool {
        ConcurrencyConfig::default() == *self
    }
}

impl Config for ConcurrencyConfig {
    fn merge_config(self, other: &Self) -> Result<Self, MergeError> {
        Ok(Self {
            solves: if other.solves == ConcurrencyConfig::default().solves {
                self.solves
            } else {
                other.solves
            },
            downloads: if other.downloads == ConcurrencyConfig::default().downloads {
                self.downloads
            } else {
                other.downloads
            },
            download_part_size: if other.download_part_size == default_download_part_size() {
                self.download_part_size
            } else {
                other.download_part_size
            },
            download_parts: if other.download_parts == default_download_parts() {
                self.download_parts
            } else {
                other.download_parts
            },
        })
    }

    fn validate(&self) -> Result<(), ValidationError> {
        if self.solves == 0 {
            return Err(ValidationError::InvalidValue(
                "solves".to_string(),
                "The number of concurrent solves must be greater than 0".to_string(),
            ));
        }

        if self.downloads == 0 {
            return Err(ValidationError::InvalidValue(
                "downloads".to_string(),
                "The number of concurrent downloads must be greater than 0".to_string(),
            ));
        }

        for (key, value) in [
            ("download-part-size", self.download_part_size),
            ("download-parts", self.download_parts as u64),
        ] {
            if value == 0 {
                return Err(ValidationError::InvalidValue(
                    key.to_string(),
                    format!("{key} must be greater than 0"),
                ));
            }
        }

        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        [
            "solves",
            "downloads",
            "download-part-size",
            "download-parts",
        ]
        .map(String::from)
        .to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml: &str) -> ConcurrencyConfig {
        toml::from_str(toml).unwrap()
    }

    #[test]
    fn download_part_settings_have_defaults() {
        let config = parse("");

        assert_eq!(config.download_part_size, 8 * 1024 * 1024);
        assert_eq!(config.download_parts, config.downloads);
    }

    #[test]
    fn download_part_settings_can_be_raised() {
        let config = parse("download-part-size = 16777216\ndownload-parts = 128\n");

        assert_eq!(config.download_part_size, 16 * 1024 * 1024);
        assert_eq!(config.download_parts, 128);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn a_later_layer_overrides_download_part_settings_it_sets() {
        let base = parse("download-part-size = 16777216\ndownload-parts = 128\n");
        let layer = parse("download-parts = 64\n");

        let merged = base.merge_config(&layer).unwrap();

        assert_eq!(merged.download_part_size, 16 * 1024 * 1024);
        assert_eq!(merged.download_parts, 64);
    }

    #[test]
    fn zero_download_part_settings_are_rejected() {
        for key in ["download-part-size", "download-parts"] {
            let err = parse(&format!("{key} = 0\n")).validate().unwrap_err();

            assert!(err.to_string().contains(key), "{err}");
        }
    }

    #[test]
    fn default_download_part_settings_are_not_saved() {
        let saved = toml::to_string(&ConcurrencyConfig::default()).unwrap();

        assert!(!saved.contains("download-part"), "{saved}");
    }
}
