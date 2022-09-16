use anyhow::Context;
use std::path::{Path, PathBuf};
use url::Url;

mod cached_package;
use crate::utils::LockFile;
pub use cached_package::CachedPackage;

/// Contains a cache of cached packages.
#[derive(Debug)]
pub struct PackageCache {
    pub path: PathBuf,
}

/// Describes a package location
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum PackageSource {
    Url(Url),
}

impl From<Url> for PackageSource {
    fn from(url: Url) -> Self {
        PackageSource::Url(url)
    }
}

impl PackageCache {
    /// Open or create a `PackageCache` at the specified location.
    pub fn new(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().canonicalize()?;
        if path.exists() && !path.is_dir() {
            anyhow::bail!("the specified path does refer to a valid directory");
        }
        std::fs::create_dir_all(&path)?;
        Ok(PackageCache { path })
    }

    /// Returns the path to the package at the given location. If the package already exists in the
    /// cache it is returned immediately. It is downloaded if it is not locally cached.
    pub async fn get(&self, package: impl Into<PackageSource>) -> anyhow::Result<CachedPackage> {
        match package.into() {
            PackageSource::Url(url) => self.get_from_url(&url).await,
        }
    }

    /// Returns the path to the package at the given location. If the package already exists in the
    /// cache it is returned immediately. It is downloaded if it is not locally cached.
    pub async fn get_from_url(&self, url: &Url) -> anyhow::Result<CachedPackage> {
        // Get the cache path in this cache
        let cache_key = self.path.join(get_url_cache_key(url));

        // If the path already exists we can assume its safe to use.
        if cache_key.exists() {
            return CachedPackage::new(&cache_key).with_context(move || {
                format!("while opening {} as CachedPackage", cache_key.display())
            });
        }

        // Otherwise lock the path
        let lock_file_path = cache_key.with_extension({
            let mut ext = cache_key
                .extension()
                .map(ToOwned::to_owned)
                .unwrap_or_default();
            ext.push(".lock");
            ext
        });
        let _lock = LockFile::new_async(&lock_file_path).await?;

        // Check again if the file exists. It could be the case that another process created the
        // cache entry while we were acquiring the lock file.
        if cache_key.exists() {
            return CachedPackage::new(&cache_key).with_context(move || {
                format!("while opening {} as CachedPackage", cache_key.display())
            });
        }

        // Download the file and store it in the cache.
        // TODO:
        std::fs::create_dir_all(&cache_key)?;
        Ok(CachedPackage::new(cache_key)?)
    }
}

/// Returns a valid relative path for the given URL that can be used as a file system cache key.
fn get_url_cache_key(url: &Url) -> PathBuf {
    if url.scheme() == "file" {
        crate::utils::url_to_cache_filename(url).into()
    } else if let Some(host) = url.host() {
        let host_str = host.to_string();
        let host = urlencoding::encode(&host_str);
        PathBuf::from(host.as_ref()).join(url.path())
    } else {
        PathBuf::from(url.path())
    }
}
