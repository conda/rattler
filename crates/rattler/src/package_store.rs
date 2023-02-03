use fxhash::FxHashMap;
use reqwest::Client;
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use url::Url;

/// A [`PackageCache`] manages a cache of extracted Conda packages on disk.
///
/// The store does not provide an implementation to get the data into the store. Instead this is
/// left up to the user when the package is requested. If the package is found in the cache it is
/// returned immediately. However, if the cache is stale a user defined function is called to
/// populate the cache. This separates the corners between caching and fetching of the content.
#[derive(Clone)]
pub struct PackageCache {
    inner: Arc<Mutex<PackageCacheInner>>,
}

/// Provides a unique identifier for packages in the cache.
/// TODO: This could not be unique over multiple subdir. How to handle?
/// TODO: Wouldn't it be better to cache based on hashes?
#[derive(Debug, Hash, Clone, Eq, PartialEq)]
struct CacheKey {
    name: String,
    version: String,
    build_string: String,
}

impl From<&PackageInfo> for CacheKey {
    fn from(pkg: &PackageInfo) -> Self {
        CacheKey {
            name: pkg.name.clone(),
            version: pkg.version.clone(),
            build_string: pkg.build_string.clone(),
        }
    }
}

impl Display for CacheKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}-{}", &self.name, &self.version, &self.build_string)
    }
}

#[derive(Default)]
struct PackageCacheInner {
    path: PathBuf,
    packages: FxHashMap<CacheKey, Arc<Mutex<Package>>>,
}

#[derive(Default)]
struct Package {
    path: Option<PathBuf>,
    inflight: Option<broadcast::Sender<Result<PathBuf, PackageCacheError>>>,
}

/// Required information about a package we want to retrieve or store in the cache.
#[derive(Debug)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub build_string: String,
}

/// An error that might be returned from one of the caching function of the [`PackageCache`].
#[derive(Debug, Clone, thiserror::Error)]
pub enum PackageCacheError {
    #[error(transparent)]
    FetchError(#[from] Arc<dyn std::error::Error + Send + Sync + 'static>),
}

impl PackageCache {
    /// Constructs a new [`PackageCache`] located at the specified path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(PackageCacheInner {
                path: path.into(),
                packages: Default::default(),
            })),
        }
    }

    /// Returns the directory that contains the specified package.
    ///
    /// If the package was previously successfully fetched and stored in the cache the directory
    /// containing the data is returned immediately. If the package was not previously fetch the
    /// filesystem is checked to see if a directory with valid package content exists. Otherwise,
    /// the user provided `fetch` function is called to populate the cache.
    ///
    /// If the package is already being fetched by another task/thread the request is coalesced. No
    /// duplicate fetch is performed.
    pub async fn get_or_fetch<F, Fut, E>(
        &self,
        pkg: PackageInfo,
        fetch: F,
    ) -> Result<PathBuf, PackageCacheError>
    where
        F: (FnOnce(PathBuf) -> Fut) + Send + 'static,
        Fut: Future<Output = Result<(), E>> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let cache_key = CacheKey::from(&pkg);

        // Get the package entry
        let (package, pkg_cache_dir) = {
            let mut inner = self.inner.lock().unwrap();
            let destination = inner.path.join(cache_key.to_string());
            let package = inner.packages.entry(cache_key).or_default().clone();
            (package, destination)
        };

        let mut rx = {
            // Only sync code in this block
            let mut inner = package.lock().unwrap();

            // If there exists an existing value in our cache, we can return that.
            if let Some(path) = inner.path.as_ref() {
                return Ok(path.clone());
            }

            // Is there an in-flight requests for the package?
            if let Some(inflight) = inner.inflight.as_ref() {
                inflight.subscribe()
            } else {
                // There is no in-flight requests so we start one!
                let (tx, rx) = broadcast::channel(1);
                inner.inflight = Some(tx.clone());

                let package = package.clone();
                tokio::spawn(async move {
                    let result = validate_or_fetch_to_cache(pkg_cache_dir.clone(), fetch).await;

                    {
                        // only sync code in this block
                        let mut package = package.lock().unwrap();
                        package.inflight = None;

                        match result {
                            Ok(_) => {
                                package.path.replace(pkg_cache_dir.clone());
                                let _ = tx.send(Ok(pkg_cache_dir));
                            }
                            Err(e) => {
                                let _ = tx.send(Err(e));
                            }
                        }
                    }
                });

                rx
            }
        };

        Ok(rx.recv().await.expect("in-flight request has died")?)
    }

    /// Returns the directory that contains the specified package.
    ///
    /// This is a convenience wrapper around `get_or_fetch` which fetches the package from the given
    /// URL if the package could not be found in the cache.
    pub async fn get_or_fetch_from_url(
        &self,
        pkg: PackageInfo,
        url: Url,
        client: Client,
    ) -> Result<PathBuf, PackageCacheError> {
        self.get_or_fetch(pkg, move |destination| async move {
            rattler_package_streaming::reqwest::tokio::extract(client, url, &destination).await
        })
        .await
    }
}

/// Validates that the package that is currently stored is a valid package and otherwise calls the
/// `fetch` method to populate the cache.
async fn validate_or_fetch_to_cache<F, Fut, E>(
    path: PathBuf,
    fetch: F,
) -> Result<(), PackageCacheError>
where
    F: FnOnce(PathBuf) -> Fut + Send,
    Fut: Future<Output = Result<(), E>> + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    // TODO: Validate the contents of the directory

    // Otherwise, defer to populate method to fill our cache.
    fetch(path)
        .await
        .map_err(|e| PackageCacheError::FetchError(Arc::new(e)))
}

#[cfg(test)]
mod test {
    use crate::package_store::{PackageCache, PackageInfo};
    use tempfile::tempdir;
    use url::Url;

    #[tokio::test]
    pub async fn test_package_cache() {
        let packages_dir = tempdir().unwrap();
        let cache = PackageCache::new(packages_dir.path());

        cache.get_or_fetch_from_url(PackageInfo {
            name: "python".to_string(),
            version: "3.11.0".to_string(),
            build_string: "h9a09f29_0_cpython".to_string(),
        },
                                    Url::parse("https://conda.anaconda.org/conda-forge/win-64/python-3.11.0-h9a09f29_0_cpython.tar.bz2").unwrap(),
                                    Default::default())
            .await
            .unwrap();
    }
}
