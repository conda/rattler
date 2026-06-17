use rattler_conda_types::utils::{InvalidPathComponentError, ensure_safe_path_component};
use rattler_conda_types::{PackageRecord, package::CondaArchiveIdentifier};
use rattler_digest::{Md5Hash, Sha256Hash};

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
        self.sha256().map(hex::encode).unwrap_or_default()
    }

    /// Renders the cache key as a file name, rejecting metadata-derived
    /// components that could escape the cache root (GHSA-h672-p7h7-97v9).
    ///
    /// This is intentionally the only way to turn a key into a path: there is no
    /// `Display`/`to_string` that could bypass the check.
    pub(crate) fn to_path_segment(&self) -> Result<String, InvalidPathComponentError> {
        // We use either the sha256 or md5 hash to disambiguate the key; if both
        // are absent we omit it.
        let hash = match (self.sha256(), self.md5()) {
            (Some(sha256), _) => format!("-{}", hex::encode(sha256)),
            (_, Some(md5)) => format!("-{}", hex::encode(md5)),
            _ => String::new(),
        };
        let segment = format!(
            "{}-{}-{}{}{}",
            &self.name, &self.version, &self.build_string, hash, self.extension
        );
        ensure_safe_path_component(&segment)?;
        Ok(segment)
    }

    /// Try to create a new cache key from a package record and a filename.
    pub fn create(record: &PackageRecord, filename: &str) -> Result<Self, CacheKeyError> {
        let archive_identifier = CondaArchiveIdentifier::try_from_filename(filename)
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

#[cfg(test)]
mod tests {
    use super::CacheKey;
    use rattler_conda_types::{PackageName, PackageRecord, VersionWithSource};

    #[test]
    fn to_path_segment_rejects_path_traversal() {
        let record = PackageRecord::new(
            PackageName::new_unchecked("demo"),
            "1.0".parse::<VersionWithSource>().unwrap(),
            r"x\..\..\..\project\.git\hooks".to_string(),
        );
        let key = CacheKey::create(&record, "demo-1.0-0.tar.bz2").unwrap();
        assert!(key.to_path_segment().is_err());
    }

    #[test]
    fn to_path_segment_accepts_well_formed_key() {
        let record = PackageRecord::new(
            PackageName::new_unchecked("demo"),
            "1.0".parse::<VersionWithSource>().unwrap(),
            "py39h6fdeb60_14".to_string(),
        );
        let key = CacheKey::create(&record, "demo-1.0-0.tar.bz2").unwrap();
        assert!(key.to_path_segment().is_ok());
    }
}
