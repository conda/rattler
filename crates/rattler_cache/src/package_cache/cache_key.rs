use rattler_conda_types::utils::{InvalidPathComponentError, ensure_safe_path_component};
use rattler_conda_types::{
    MatchSpec, PackageName, PackageRecord, VersionWithSource, match_spec::Matches,
    package::{BuildString, CondaArchiveIdentifier},
};
use rattler_digest::{Md5Hash, Sha256, Sha256Hash, compute_bytes_digest, compute_url_digest};
use std::path::Path;

/// Provides a unique identifier for packages in the cache.
/// TODO: This could not be unique over multiple subdir. How to handle?
#[derive(Debug, Hash, Clone, Eq, PartialEq)]
pub struct CacheKey {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) build_string: String,
    pub(crate) sha256: Option<Sha256Hash>,
    pub(crate) md5: Option<Md5Hash>,
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

    /// Adds a md5 hash of the archive.
    pub fn with_md5(mut self, md5: Md5Hash) -> Self {
        self.md5 = Some(md5);
        self
    }

    /// Potentially adds a md5 hash of the archive.
    pub fn with_opt_md5(mut self, md5: Option<Md5Hash>) -> Self {
        self.md5 = md5;
        self
    }

    /// Adds a hash of the Url to the cache key
    pub fn with_url(mut self, url: url::Url) -> Self {
        let url_hash = compute_url_digest::<Sha256>(url);
        self.origin_hash = Some(hex::encode(url_hash));
        self
    }

    /// Adds a hash of the Path to the cache key
    pub fn with_path(mut self, path: &Path) -> Self {
        let path_hash = compute_bytes_digest::<Sha256>(path.as_os_str().as_encoded_bytes());
        self.origin_hash = Some(hex::encode(path_hash));
        self
    }
}

impl CacheKey {
    /// Renders the cache key as a directory name, rejecting metadata-derived
    /// components that could escape the cache root (GHSA-h672-p7h7-97v9).
    ///
    /// This is intentionally the only way to turn a key into a path: there is no
    /// `Display`/`to_string` that could bypass the check.
    pub(crate) fn to_path_segment(&self) -> Result<String, InvalidPathComponentError> {
        let segment = match &self.origin_hash {
            Some(url_hash) => format!(
                "{}-{}-{}-{}",
                &self.name, &self.version, &self.build_string, url_hash
            ),
            None => format!("{}-{}-{}", &self.name, &self.version, &self.build_string),
        };
        ensure_safe_path_component(&segment)?;
        Ok(segment)
    }

    /// Return the sha256 hash of the package if it is known.
    pub fn sha256(&self) -> Option<Sha256Hash> {
        self.sha256
    }

    /// Return the md5 hash of the package if it is known.
    pub fn md5(&self) -> Option<Md5Hash> {
        self.md5
    }

    /// Matches the parts of a [`MatchSpec`] represented by a cache key.
    ///
    /// A cache key contains name, version, build string and optional hashes.
    /// Specifications constraining any other package metadata cannot be
    /// evaluated reliably and therefore do not match.
    pub(crate) fn matches_spec(&self, spec: &MatchSpec) -> bool {
        if spec.build_number.is_some()
            || spec.file_name.is_some()
            || spec.extras.is_some()
            || spec.flags.is_some()
            || spec.channel.is_some()
            || spec.subdir.is_some()
            || spec.namespace.is_some()
            || spec.url.is_some()
            || spec.license.is_some()
            || spec.license_family.is_some()
            || spec.condition.is_some()
            || spec.track_features.is_some()
        {
            return false;
        }

        let Ok(name) = self.name.parse::<PackageName>() else {
            return false;
        };
        let Ok(version) = self.version.parse::<VersionWithSource>() else {
            return false;
        };
        let mut record = PackageRecord::new(
            name,
            version,
            BuildString::new_unchecked(self.build_string.clone()),
        );
        record.sha256 = self.sha256;
        record.md5 = self.md5;
        spec.matches(&record)
    }
}

impl From<CondaArchiveIdentifier> for CacheKey {
    fn from(pkg: CondaArchiveIdentifier) -> Self {
        CacheKey {
            name: pkg.identifier.name,
            version: pkg.identifier.version,
            build_string: pkg.identifier.build_string,
            sha256: None,
            md5: None,
            origin_hash: None,
        }
    }
}

impl From<&PackageRecord> for CacheKey {
    fn from(record: &PackageRecord) -> Self {
        Self {
            name: record.name.as_normalized().to_string(),
            version: record.version.to_string(),
            build_string: record.build.to_string(),
            sha256: record.sha256,
            md5: record.md5,
            origin_hash: None,
        }
    }
}
