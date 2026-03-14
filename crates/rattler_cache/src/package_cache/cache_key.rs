use rattler_conda_types::package::CondaArchiveIdentifier;
use rattler_conda_types::PackageRecord;
use rattler_digest::{compute_bytes_digest, compute_url_digest, Md5Hash, Sha256, Sha256Hash};
use std::{
    fmt::{Display, Formatter},
    path::Path,
};

/// Provides a unique identifier for packages in the cache.
///
/// The key includes the package name, version, build string, and optionally the
/// subdirectory (platform identifier, e.g. `linux-64`, `osx-arm64`). Including
/// the subdir prevents cache collisions between packages with identical
/// name/version/build coordinates from different platforms.
#[derive(Debug, Hash, Clone, Eq, PartialEq)]
pub struct CacheKey {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) build_string: String,
    /// included in the cache directory name to avoid collisions across subdirectories.
    pub(crate) subdir: Option<String>,
    pub(crate) sha256: Option<Sha256Hash>,
    pub(crate) md5: Option<Md5Hash>,
    pub(crate) origin_hash: Option<String>,
}

impl CacheKey {
    /// Adds the subdirectory (platform identifier, e.g. `linux-64`) to the key.
    pub fn with_subdir(mut self, subdir: impl Into<String>) -> Self {
        self.subdir = Some(subdir.into());
        self
    }

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
        self.origin_hash = Some(format!("{url_hash:x}"));
        self
    }

    /// Adds a hash of the Path to the cache key
    pub fn with_path(mut self, path: &Path) -> Self {
        let path_hash = compute_bytes_digest::<Sha256>(path.as_os_str().as_encoded_bytes());
        self.origin_hash = Some(format!("{path_hash:x}"));
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
}

impl From<CondaArchiveIdentifier> for CacheKey {
    fn from(pkg: CondaArchiveIdentifier) -> Self {
        CacheKey {
            name: pkg.identifier.name,
            version: pkg.identifier.version,
            build_string: pkg.identifier.build_string,
            // Subdir is not available from the archive filename alone.
            subdir: None,
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
            build_string: record.build.clone(),
            // Include the subdir so that packages with the same
            // name/version/build from different platforms (e.g. linux-64 vs
            // osx-arm64) do not collide in the cache.
            subdir: Some(record.subdir.clone()),
            sha256: record.sha256,
            md5: record.md5,
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
            None => match &self.subdir {
                // Include the subdir in the directory name so that packages
                // from different platforms with identical name/version/build do
                // not share the same cache entry.
                Some(subdir) => write!(
                    f,
                    "{}-{}-{}-{}",
                    &self.name, &self.version, &self.build_string, subdir
                ),
                None => write!(f, "{}-{}-{}", &self.name, &self.version, &self.build_string),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rattler_conda_types::{NoArchType, PackageName, VersionWithSource};
    use std::collections::BTreeMap;
    use std::str::FromStr;

    fn make_record(name: &str, version: &str, build: &str, subdir: &str) -> PackageRecord {
        PackageRecord {
            name: PackageName::new_unchecked(name),
            version: VersionWithSource::from_str(version).unwrap(),
            build: build.to_string(),
            build_number: 0,
            subdir: subdir.to_string(),
            arch: None,
            platform: None,
            depends: vec![],
            constrains: vec![],
            track_features: vec![],
            features: None,
            noarch: NoArchType::none(),
            license: None,
            license_family: None,
            legacy_bz2_md5: None,
            legacy_bz2_size: None,
            md5: None,
            sha256: None,
            size: None,
            timestamp: None,
            purls: None,
            run_exports: None,
            python_site_packages_path: None,
            experimental_extra_depends: BTreeMap::default(),
        }
    }

    #[test]
    fn test_display_no_subdir_no_hash() {
        let key = CacheKey {
            name: "zlib".to_string(),
            version: "1.3.1".to_string(),
            build_string: "hb9d3cd8_2".to_string(),
            subdir: None,
            sha256: None,
            md5: None,
            origin_hash: None,
        };
        assert_eq!(key.to_string(), "zlib-1.3.1-hb9d3cd8_2");
    }

    #[test]
    fn test_display_with_subdir() {
        let key = CacheKey {
            name: "zlib".to_string(),
            version: "1.3.1".to_string(),
            build_string: "hb9d3cd8_2".to_string(),
            subdir: Some("linux-64".to_string()),
            sha256: None,
            md5: None,
            origin_hash: None,
        };
        assert_eq!(key.to_string(), "zlib-1.3.1-hb9d3cd8_2-linux-64");
    }

    #[test]
    fn test_display_origin_hash_takes_precedence_over_subdir() {
        let key = CacheKey {
            name: "zlib".to_string(),
            version: "1.3.1".to_string(),
            build_string: "hb9d3cd8_2".to_string(),
            subdir: Some("linux-64".to_string()),
            sha256: None,
            md5: None,
            origin_hash: Some("abc123".to_string()),
        };
        // origin_hash wins — subdir is not appended separately
        assert_eq!(key.to_string(), "zlib-1.3.1-hb9d3cd8_2-abc123");
    }

    #[test]
    fn test_from_package_record_includes_subdir() {
        let record = make_record("zlib", "1.3.1", "hb9d3cd8_2", "linux-64");
        let key = CacheKey::from(&record);
        assert_eq!(key.subdir, Some("linux-64".to_string()));
        assert_eq!(key.to_string(), "zlib-1.3.1-hb9d3cd8_2-linux-64");
    }

    #[test]
    fn test_cross_platform_keys_are_distinct() {
        // Same name/version/build but different subdir must not collide.
        let linux = make_record("mylib", "1.0.0", "h1234_0", "linux-64");
        let osx = make_record("mylib", "1.0.0", "h1234_0", "osx-arm64");

        let key_linux = CacheKey::from(&linux);
        let key_osx = CacheKey::from(&osx);

        assert_ne!(
            key_linux.to_string(),
            key_osx.to_string(),
            "keys from different subdirs must produce different cache directory names"
        );
        assert_ne!(key_linux, key_osx);
    }

    #[test]
    fn test_from_archive_identifier_has_no_subdir() {
        let id = CondaArchiveIdentifier::try_from_filename("zlib-1.3.1-hb9d3cd8_2.conda").unwrap();
        let key = CacheKey::from(id);
        assert_eq!(key.subdir, None);
        // Falls back to the subdir-less format for backward compatibility
        assert_eq!(key.to_string(), "zlib-1.3.1-hb9d3cd8_2");
    }

    #[test]
    fn test_with_subdir_builder() {
        let id = CondaArchiveIdentifier::try_from_filename("zlib-1.3.1-hb9d3cd8_2.conda").unwrap();
        let key = CacheKey::from(id).with_subdir("win-64");
        assert_eq!(key.to_string(), "zlib-1.3.1-hb9d3cd8_2-win-64");
    }
}
