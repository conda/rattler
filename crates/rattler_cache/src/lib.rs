use std::path::PathBuf;

pub mod package_cache;

pub mod validation;

mod consts;
pub use consts::{PACKAGE_CACHE_DIR, REPODATA_CACHE_DIR};

/// Returns the default cache directory used by rattler.
pub fn default_cache_dir() -> anyhow::Result<PathBuf> {
    Ok(dirs::cache_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine cache directory for current platform"))?
        .join("rattler/cache"))
}
