use rattler_conda_types::{package::ArchiveIdentifier, PackageRecord};
use rattler_digest::{Md5Hash, Sha256Hash};
use std::fmt::{Display, Formatter};

/// Provides a unique identifier for packages in the cache.
#[derive(Debug, Hash, Clone, Eq, PartialEq)]
pub struct CacheKey {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) build_string: String,
    pub(crate) sha256: Option<Sha256Hash>,
    pub(crate) md5: Option<Md5Hash>,
    pub(crate) extension: String,
}

impl CacheKey {
    /// Potentially adds a sha256 hash of the archive.
    pub fn with_opt_sha256(mut self, sha256: Option<Sha256Hash>) -> Self {
        self.sha256 = sha256;
        self
    }

    /// Potentially adds a md5 hash of the archive.
    pub fn with_opt_md5(mut self, md5: Option<Md5Hash>) -> Self {
        self.md5 = md5;
        self
    }
}

impl CacheKey {
    /// Return the sha256 hash of the package if it is known.
    pub fn sha256(&self) -> Option<Sha256Hash> {
        self.sha256
    }

    /// Return the md5 hash of the package if it is known.
    pub fn md5(&self) -> Option<Md5Hash> {
        self.md5
    }

    /// Return the sha256 hash string of the package if it is known.
    pub fn sha256_str(&self) -> String {
        self.sha256()
            .map(|hash| format!("{hash:x}"))
            .unwrap_or_default()
    }

    /// Try to create a new cache key from a package record and a filename.
    pub fn create(record: &PackageRecord, filename: &str) -> Result<Self, CacheKeyError> {
        let archive_identifier = ArchiveIdentifier::try_from_filename(filename)
            .ok_or_else(|| CacheKeyError::InvalidArchiveIdentifier(filename.to_string()))?;

        Ok(Self {
            name: record.name.as_normalized().to_string(),
            version: record.version.to_string(),
            build_string: record.build.clone(),
            sha256: record.sha256,
            md5: record.md5,
            extension: archive_identifier.archive_type.extension().to_string(),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CacheKeyError {
    #[error("could not identify the archive type from the name: {0}")]
    InvalidArchiveIdentifier(String),
}

impl Display for CacheKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // we need to use either sha256 or md5 hash to display the key
        // if both are none, we ignore them
        let display_key = match (self.sha256(), self.md5()) {
            (Some(sha256), _) => format!("-{sha256:x}"),
            (_, Some(md5)) => format!("-{md5:x}"),
            _ => "".to_string(),
        };

        write!(
            f,
            "{}-{}-{}{}{}",
            &self.name, &self.version, &self.build_string, display_key, self.extension
        )
    }
}
