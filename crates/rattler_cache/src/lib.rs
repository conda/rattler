use anyhow::Ok;
use std::path::PathBuf;

pub mod package_cache;

pub mod validation;

mod consts;
pub use consts::{PACKAGE_CACHE_DIR, REPODATA_CACHE_DIR};

/// Determines the default cache directory for rattler.
/// It first checks the environment variable `RATTLER_CACHE_DIR`.
/// If not set, it falls back to the environment variable `XDG_CACHE_HOME`.
/// If not set, it falls back to the standard cache directory provided by `dirs::cache_dir()`.
pub fn default_cache_dir() -> anyhow::Result<PathBuf> {
    Ok(std::env::var("RATTLER_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|_| {
            std::env::var("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .or_else(|_| {
                    dirs::cache_dir().ok_or_else(|| {
                        anyhow::anyhow!("could not determine cache directory for current platform")
                    })
                })
        })?
        .join("rattler/cache"))
}
