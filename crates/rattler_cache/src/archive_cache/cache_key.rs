use rattler_conda_types::{package::ArchiveIdentifier, PackageRecord};
use rattler_digest::Sha256Hash;
use std::fmt::{Display, Formatter};

/// Provides a unique identifier for packages in the cache.
/// TODO: This could not be unique over multiple subdir. How to handle?
#[derive(Debug, Hash, Clone, Eq, PartialEq)]
pub struct CacheKey {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) build_string: String,
    pub(crate) sha256: Option<Sha256Hash>,
    pub(crate) extension: String,
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
}

impl CacheKey {
    /// Return the sha256 hash of the package if it is known.
    pub fn sha256(&self) -> Option<Sha256Hash> {
        self.sha256
    }

    /// Return the sha256 hash string of the package if it is known.
    pub fn sha256_str(&self) -> String {
        self.sha256()
            .map(|hash| format!("{hash:x}"))
            .unwrap_or_default()
    }

    /// Try to create a new cache key from a package record and a filename.
    pub fn new(record: &PackageRecord, filename: &str) -> Result<Self, CacheKeyError> {
        let archive_identifier = ArchiveIdentifier::try_from_filename(filename)
            .ok_or_else(|| CacheKeyError::InvalidArchiveIdentifier(filename.to_string()))?;

        Ok(Self {
            name: record.name.as_normalized().to_string(),
            version: record.version.to_string(),
            build_string: record.build.clone(),
            sha256: record.sha256,
            extension: archive_identifier.archive_type.extension().to_string(),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CacheKeyError {
    #[error("Could not identify the archive type from the name: {0}")]
    InvalidArchiveIdentifier(String),
}

impl Display for CacheKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}-{}-{}-{}{}",
            &self.name,
            &self.version,
            &self.build_string,
            self.sha256_str(),
            self.extension
        )
    }
}
