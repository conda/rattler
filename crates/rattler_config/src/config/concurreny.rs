use serde::{Deserialize, Serialize};

use crate::config::Config;

// Making the default values part of pixi_config to allow for printing the
// default settings in the future.
/// The default maximum number of concurrent solves that can be run at once.
/// Defaulting to the number of CPUs available.
fn default_max_concurrent_solves() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// The default maximum number of concurrent downloads that can be run at once.
/// 50 is a reasonable default for the number of concurrent downloads.
/// More verification is needed to determine the optimal number.
fn default_max_concurrent_downloads() -> usize {
    50
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
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        Self {
            solves: default_max_concurrent_solves(),
            downloads: default_max_concurrent_downloads(),
        }
    }
}

impl ConcurrencyConfig {
    pub fn is_default(&self) -> bool {
        ConcurrencyConfig::default() == *self
    }
}

impl Config for ConcurrencyConfig {
    fn get_extension_name(&self) -> String {
        "concurrency".to_string()
    }

    fn merge_config(self, other: &Self) -> Result<Self, miette::Error> {
        Ok(Self {
            solves: if other.solves != ConcurrencyConfig::default().solves {
                other.solves
            } else {
                self.solves
            },
            downloads: if other.downloads != ConcurrencyConfig::default().downloads {
                other.downloads
            } else {
                self.downloads
            },
        })
    }

    fn validate(&self) -> Result<(), miette::Error> {
        if self.solves == 0 || self.downloads == 0 {
            return Err(miette::miette!("Concurrency values must be greater than 0"));
        }
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        vec!["solves".to_string(), "downloads".to_string()]
    }
}
