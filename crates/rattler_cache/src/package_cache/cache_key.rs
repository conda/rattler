use rattler_conda_types::package::ArchiveIdentifier;
use rattler_conda_types::PackageRecord;
use rattler_digest::{compute_bytes_digest, compute_url_digest, Sha256, Sha256Hash};
use std::{
    fmt::{Display, Formatter},
    path::Path,
};

/// Provides a unique identifier for packages in the cache.
/// TODO: This could not be unique over multiple subdir. How to handle?
#[derive(Debug, Hash, Clone, Eq, PartialEq)]
pub struct CacheKey {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) build_string: String,
    pub(crate) sha256: Option<Sha256Hash>,
    pub(crate) origin_hash: Option<String>,
}

impl CacheKey {
    /// Adds a sha256 hash of the archive.
    pub fn with_sha256(mut self, sha256: Sha256Hash) -> Self {
        self.sha256 = Some(sha256);
        self
    }

    /// Potentially adds a sha256 hash of the archive.
    pub fn with_opt_sha256(mut self, sha256: Option<Sha256Hash>) -> Self {
        self.sha256 = sha256;
        self
    }

    /// Adds a hash of the Url to the cache key
    pub fn with_url(mut self, url: url::Url) -> Self {
        let url_hash = compute_url_digest::<Sha256>(url);
        self.origin_hash = Some(format!("{url_hash:x}"));
        self
    }

    /// Adds a hash of the Path to the cache key
    pub fn with_path(mut self, path: &Path) -> Self {
        let path_hash = compute_bytes_digest::<Sha256>(path.to_string_lossy().as_bytes());
        self.origin_hash = Some(format!("{path_hash:x}"));
        self
    }
}

impl CacheKey {
    /// Return the sha256 hash of the package if it is known.
    pub fn sha256(&self) -> Option<Sha256Hash> {
        self.sha256
    }
}

impl From<ArchiveIdentifier> for CacheKey {
    fn from(pkg: ArchiveIdentifier) -> Self {
        CacheKey {
            name: pkg.name,
            version: pkg.version,
            build_string: pkg.build_string,
            sha256: None,
            origin_hash: None,
        }
    }
}

impl From<&PackageRecord> for CacheKey {
    fn from(record: &PackageRecord) -> Self {
        Self {
            name: record.name.as_normalized().to_string(),
            version: record.version.to_string(),
            build_string: record.build.clone(),
            sha256: record.sha256,
            origin_hash: None,
        }
    }
}

impl Display for CacheKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.origin_hash {
            Some(url_hash) => write!(
                f,
                "{}-{}-{}-{}",
                &self.name, &self.version, &self.build_string, url_hash
            ),
            None => write!(f, "{}-{}-{}", &self.name, &self.version, &self.build_string),
        }
    }
}
