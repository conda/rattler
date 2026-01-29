#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
pub mod package_cache;
#[cfg(not(target_arch = "wasm32"))]
pub mod run_exports_cache;

#[cfg(not(target_arch = "wasm32"))]
pub mod validation;

mod consts;
pub use consts::{PACKAGE_CACHE_DIR, REPODATA_CACHE_DIR, RUN_EXPORTS_CACHE_DIR};

/// Determines the default cache directory for rattler.
/// It first checks the environment variable `RATTLER_CACHE_DIR`.
/// If not set, it falls back to the standard cache directory provided by `dirs::cache_dir()/rattler/cache`.
#[cfg(not(target_arch = "wasm32"))]
pub fn default_cache_dir() -> anyhow::Result<PathBuf> {
    std::env::var("RATTLER_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|_| {
            dirs::cache_dir()
                .ok_or_else(|| {
                    anyhow::anyhow!("could not determine cache directory for current platform")
                })
                // Append `rattler/cache` to the cache directory
                .map(|mut p| {
                    p.push("rattler");
                    p.push("cache");
                    p
                })
        })
}

/// Creates the cache directory if it doesn't exist and excludes it from backups.
///
/// This function:
/// 1. Creates the directory and all parent directories if they don't exist
/// 2. Creates a `CACHEDIR.TAG` file to exclude from backup tools (borg, restic, etc.)
/// 3. On macOS, marks the directory as excluded from Time Machine
///
/// This is idempotent - calling it multiple times on the same directory is safe.
#[cfg(not(target_arch = "wasm32"))]
pub fn ensure_cache_dir(path: &Path) -> std::io::Result<()> {
    fs_err::create_dir_all(path)?;
    rattler_conda_types::backup::exclude_from_backups(path)?;
    Ok(())
}
