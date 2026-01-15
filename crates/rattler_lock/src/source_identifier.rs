use std::{
    fmt::{Display, Formatter, Write as _},
    str::FromStr,
};

use rattler_conda_types::PackageName;
use rattler_digest::{Sha256, Sha256Hash};
use serde_with::{DeserializeFromStr, SerializeDisplay};
use thiserror::Error;

use crate::{CondaSourceData, UrlOrPath};

/// A unique identifier for a source package in the lock file.
///
/// This type represents the format `<name>[<hash>] @ <location>` which is used
/// to uniquely identify source packages. The hash is computed from the package
/// record to disambiguate packages at the same location with different configurations.
///
/// # Examples
///
/// ```text
/// numba-cuda[9f3c2a7b] @ .
/// numba-cuda[9f3c2a7b] @ https://example.com/pkgs/...
/// numba-cuda[9f3c2a7b] @ git+https://host/org/repo@main
/// ```
#[derive(Debug, Clone, Eq, PartialEq, Hash, SerializeDisplay, DeserializeFromStr)]
pub struct SourceIdentifier {
    /// The name of the package.
    name: PackageName,

    /// A short hash (8 hex characters) computed from the package record.
    /// This is used to disambiguate packages at the same location.
    hash: String,

    /// The location of the source package (URL or path).
    location: UrlOrPath,
}

/// The number of hex characters to use for the short hash.
const SHORT_HASH_LENGTH: usize = 8;

impl SourceIdentifier {
    /// Creates a new source identifier.
    ///
    /// # Arguments
    ///
    /// * `name` - The package name
    /// * `hash` - A short hash string (typically 8 hex characters)
    /// * `location` - The location of the source package
    pub fn new(name: PackageName, hash: impl Into<String>, location: UrlOrPath) -> Self {
        Self {
            name,
            hash: hash.into(),
            location,
        }
    }

    /// Creates a source identifier from a `CondaSourceData`.
    ///
    /// The hash is computed from the package record fields that uniquely identify
    /// the package configuration (name, version, build, `build_number`, subdir, and variants).
    pub fn from_source_data(source_data: &CondaSourceData) -> Self {
        let hash = compute_source_hash(source_data);
        let short_hash = format_short_hash(&hash);

        Self {
            name: source_data.package_record.name.clone(),
            hash: short_hash,
            location: source_data.location.clone(),
        }
    }

    /// Returns the package name.
    pub fn name(&self) -> &PackageName {
        &self.name
    }

    /// Returns the hash.
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// Returns the location.
    pub fn location(&self) -> &UrlOrPath {
        &self.location
    }

    /// Consumes this identifier and returns its parts.
    pub fn into_parts(self) -> (PackageName, String, UrlOrPath) {
        (self.name, self.hash, self.location)
    }
}

impl Display for SourceIdentifier {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}[{}] @ {}",
            self.name.as_source(),
            self.hash,
            self.location
        )
    }
}

/// Error type for parsing a [`SourceIdentifier`] from a string.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum ParseSourceIdentifierError {
    /// Missing the opening bracket `[` for the hash.
    #[error("missing '[' after package name")]
    MissingOpenBracket,

    /// Missing the closing bracket `]` for the hash.
    #[error("missing ']' after hash")]
    MissingCloseBracket,

    /// Missing the ` @ ` separator between the identifier and location.
    #[error("missing ' @ ' separator")]
    MissingSeparator,

    /// Invalid package name.
    #[error("invalid package name: {0}")]
    InvalidPackageName(#[from] rattler_conda_types::InvalidPackageNameError),

    /// Invalid location.
    #[error("invalid location: {0}")]
    InvalidLocation(#[from] crate::url_or_path::PathOrUrlError),

    /// Empty hash.
    #[error("hash cannot be empty")]
    EmptyHash,
}

/// Computes a SHA-256 hash from the source package data.
///
/// The hash is computed from fields that uniquely identify the package configuration:
/// - name
/// - version
/// - build
/// - `build_number`
/// - subdir
/// - variants (sorted for determinism)
fn compute_source_hash(source_data: &CondaSourceData) -> Sha256Hash {
    use rattler_digest::digest::Digest;

    let record = &source_data.package_record;
    let mut hasher = Sha256::default();

    // Hash the identifying fields
    hasher.update(record.name.as_normalized().as_bytes());
    hasher.update(b"\0");
    hasher.update(record.version.to_string().as_bytes());
    hasher.update(b"\0");
    hasher.update(record.build.as_bytes());
    hasher.update(b"\0");
    hasher.update(record.build_number.to_string().as_bytes());
    hasher.update(b"\0");
    hasher.update(record.subdir.as_bytes());
    hasher.update(b"\0");

    // Hash variants (sorted for determinism)
    for (key, value) in &source_data.variants {
        hasher.update(key.as_bytes());
        hasher.update(b"=");
        hasher.update(value.to_string().as_bytes());
        hasher.update(b"\0");
    }

    hasher.finalize()
}

/// Formats a SHA-256 hash as a short hex string.
fn format_short_hash(hash: &Sha256Hash) -> String {
    let mut result = String::with_capacity(SHORT_HASH_LENGTH);
    for byte in hash.iter().take(SHORT_HASH_LENGTH / 2) {
        write!(result, "{byte:02x}").expect("writing to string cannot fail");
    }
    result
}

impl FromStr for SourceIdentifier {
    type Err = ParseSourceIdentifierError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Find the opening bracket
        let open_bracket = s
            .find('[')
            .ok_or(ParseSourceIdentifierError::MissingOpenBracket)?;

        // Find the closing bracket
        let close_bracket = s
            .find(']')
            .ok_or(ParseSourceIdentifierError::MissingCloseBracket)?;

        // Ensure brackets are in correct order
        if close_bracket <= open_bracket {
            return Err(ParseSourceIdentifierError::MissingCloseBracket);
        }

        // Extract the name part (before the opening bracket)
        let name_str = &s[..open_bracket];
        let name = PackageName::from_str(name_str)?;

        // Extract the hash (between brackets)
        let hash = &s[open_bracket + 1..close_bracket];
        if hash.is_empty() {
            return Err(ParseSourceIdentifierError::EmptyHash);
        }

        // The rest should be " @ <location>"
        let remainder = &s[close_bracket + 1..];
        let location_str = remainder
            .strip_prefix(" @ ")
            .ok_or(ParseSourceIdentifierError::MissingSeparator)?;

        let location = UrlOrPath::from_str(location_str)?;

        Ok(Self {
            name,
            hash: hash.to_string(),
            location,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_relative_path() {
        let id: SourceIdentifier = "numba-cuda[9f3c2a7b] @ .".parse().unwrap();
        assert_eq!(id.name().as_source(), "numba-cuda");
        assert_eq!(id.hash(), "9f3c2a7b");
        assert_eq!(id.location().as_str(), ".");
    }

    #[test]
    fn test_parse_url() {
        let id: SourceIdentifier = "my-package[abcd1234] @ https://example.com/pkgs/source"
            .parse()
            .unwrap();
        assert_eq!(id.name().as_source(), "my-package");
        assert_eq!(id.hash(), "abcd1234");
        assert_eq!(id.location().as_str(), "https://example.com/pkgs/source");
    }

    #[test]
    fn test_parse_git_url() {
        let id: SourceIdentifier = "my-pkg[deadbeef] @ git+https://github.com/org/repo@main"
            .parse()
            .unwrap();
        assert_eq!(id.name().as_source(), "my-pkg");
        assert_eq!(id.hash(), "deadbeef");
        assert_eq!(
            id.location().as_str(),
            "git+https://github.com/org/repo@main"
        );
    }

    #[test]
    fn test_display_roundtrip() {
        let original = "numba-cuda[9f3c2a7b] @ .";
        let id: SourceIdentifier = original.parse().unwrap();
        assert_eq!(id.to_string(), original);
    }

    #[test]
    fn test_display_url_roundtrip() {
        let original = "my-package[abcd1234] @ https://example.com/pkgs/source";
        let id: SourceIdentifier = original.parse().unwrap();
        assert_eq!(id.to_string(), original);
    }

    #[test]
    fn test_missing_open_bracket() {
        let result: Result<SourceIdentifier, _> = "numba-cuda9f3c2a7b] @ .".parse();
        assert!(matches!(
            result,
            Err(ParseSourceIdentifierError::MissingOpenBracket)
        ));
    }

    #[test]
    fn test_missing_close_bracket() {
        let result: Result<SourceIdentifier, _> = "numba-cuda[9f3c2a7b @ .".parse();
        assert!(matches!(
            result,
            Err(ParseSourceIdentifierError::MissingCloseBracket)
        ));
    }

    #[test]
    fn test_missing_separator() {
        let result: Result<SourceIdentifier, _> = "numba-cuda[9f3c2a7b].".parse();
        assert!(matches!(
            result,
            Err(ParseSourceIdentifierError::MissingSeparator)
        ));
    }

    #[test]
    fn test_empty_hash() {
        let result: Result<SourceIdentifier, _> = "numba-cuda[] @ .".parse();
        assert!(matches!(result, Err(ParseSourceIdentifierError::EmptyHash)));
    }

    #[test]
    fn test_invalid_package_name() {
        // A name with invalid characters should fail to parse
        let result: Result<SourceIdentifier, _> = "invalid name with spaces[hash] @ .".parse();
        assert!(
            matches!(
                result,
                Err(ParseSourceIdentifierError::InvalidPackageName(_))
            ),
            "expected InvalidPackageName error, got: {result:?}"
        );
    }

    #[test]
    fn test_serde_roundtrip() {
        let id = SourceIdentifier::new(
            PackageName::from_str("my-package").unwrap(),
            "abcd1234",
            UrlOrPath::from_str(".").unwrap(),
        );

        let serialized = serde_yaml::to_string(&id).unwrap();
        let deserialized: SourceIdentifier = serde_yaml::from_str(&serialized).unwrap();

        assert_eq!(id, deserialized);
    }

    #[test]
    fn test_from_source_data() {
        use std::collections::BTreeMap;

        use rattler_conda_types::{PackageRecord, VersionWithSource};

        use crate::CondaSourceData;

        let mut package_record = PackageRecord::new(
            PackageName::from_str("numba-cuda").unwrap(),
            VersionWithSource::from_str("0.23.0").unwrap(),
            "py310h3ca6f64_0".to_string(),
        );
        package_record.subdir = "linux-64".to_string();

        let source_data = CondaSourceData {
            package_record,
            location: UrlOrPath::from_str(".").unwrap(),
            variants: BTreeMap::new(),
            package_build_source: None,
            sources: BTreeMap::new(),
        };

        let id = SourceIdentifier::from_source_data(&source_data);

        assert_eq!(id.name().as_source(), "numba-cuda");
        assert_eq!(id.hash().len(), 8);
        assert_eq!(id.location().as_str(), ".");

        // Verify the hash is deterministic
        let id2 = SourceIdentifier::from_source_data(&source_data);
        assert_eq!(id.hash(), id2.hash());
    }

    #[test]
    fn test_from_source_data_with_variants() {
        use std::collections::BTreeMap;

        use rattler_conda_types::{PackageRecord, VersionWithSource};

        use crate::{CondaSourceData, VariantValue};

        let mut package_record = PackageRecord::new(
            PackageName::from_str("numba-cuda").unwrap(),
            VersionWithSource::from_str("0.23.0").unwrap(),
            "py310h3ca6f64_0".to_string(),
        );
        package_record.subdir = "linux-aarch64".to_string();

        let mut variants = BTreeMap::new();
        variants.insert(
            "python".to_string(),
            VariantValue::String("3.10.*".to_string()),
        );
        variants.insert(
            "target_platform".to_string(),
            VariantValue::String("linux-aarch64".to_string()),
        );

        let source_data = CondaSourceData {
            package_record,
            location: UrlOrPath::from_str(".").unwrap(),
            variants,
            package_build_source: None,
            sources: BTreeMap::new(),
        };

        let id = SourceIdentifier::from_source_data(&source_data);

        // The hash should be different from the one without variants
        assert_eq!(id.name().as_source(), "numba-cuda");
        assert_eq!(id.hash().len(), 8);
    }

    #[test]
    fn compute_test_data_hashes() {
        use std::collections::BTreeMap;

        use rattler_conda_types::{PackageRecord, VersionWithSource};

        use crate::CondaSourceData;

        fn print_hash(name: &str, version: &str, build: &str, subdir: &str, location: &str) {
            let mut package_record = PackageRecord::new(
                PackageName::from_str(name).unwrap(),
                VersionWithSource::from_str(version).unwrap(),
                build.to_string(),
            );
            package_record.subdir = subdir.to_string();

            let source_data = CondaSourceData {
                package_record,
                location: UrlOrPath::from_str(location).unwrap(),
                variants: BTreeMap::new(),
                package_build_source: None,
                sources: BTreeMap::new(),
            };

            let id = SourceIdentifier::from_source_data(&source_data);
            println!("  {name}: {id}");
        }

        println!("Computed hashes for test data:");

        // sources-lock.yml, derived-channel-lock.yml
        print_hash(
            "child-package",
            "0.1.0",
            "pyhbf21a9e_0",
            "noarch",
            "child-package",
        );

        // conda-path-lock.yml
        print_hash(
            "minimal-project",
            "0.1",
            "first",
            "linux-64",
            "../minimal-project",
        );
        print_hash(
            "minimal-project",
            "0.1",
            "first",
            "win-64",
            "../minimal-project",
        );
        print_hash(
            "minimal-project",
            "0.1",
            "second",
            "win-64",
            "../minimal-project",
        );
        print_hash(
            "a-python-project",
            "0.1",
            "py38",
            "noarch",
            "../a-python-project",
        );
        print_hash(
            "b-python-project",
            "0.1",
            "h398123",
            "noarch",
            "../a-python-project",
        );

        // pixi-build-*.yml files
        print_hash(
            "pixi-build-package",
            "1.0.0",
            "pyhbf21a9e_0",
            "noarch",
            "pixi-build-package",
        );
        print_hash(
            "pixi-url-package",
            "2.0.0",
            "pyhbf21a9e_0",
            "noarch",
            "pixi-url-package",
        );
        print_hash(
            "pixi-tag-package",
            "1.2.0",
            "pyhbf21a9e_0",
            "noarch",
            "pixi-tag-package",
        );
        print_hash(
            "pixi-rev-package",
            "0.5.0",
            "pyhbf21a9e_0",
            "noarch",
            "pixi-rev-package",
        );
    }

    #[test]
    fn test_different_variants_produce_different_hashes() {
        use std::collections::BTreeMap;

        use rattler_conda_types::{PackageRecord, VersionWithSource};

        use crate::{CondaSourceData, VariantValue};

        let mut package_record = PackageRecord::new(
            PackageName::from_str("my-package").unwrap(),
            VersionWithSource::from_str("1.0.0").unwrap(),
            "h0000000_0".to_string(),
        );
        package_record.subdir = "linux-64".to_string();

        // First variant: python 3.10
        let mut variants1 = BTreeMap::new();
        variants1.insert(
            "python".to_string(),
            VariantValue::String("3.10".to_string()),
        );

        let source_data1 = CondaSourceData {
            package_record: package_record.clone(),
            location: UrlOrPath::from_str(".").unwrap(),
            variants: variants1,
            package_build_source: None,
            sources: BTreeMap::new(),
        };

        // Second variant: python 3.11
        let mut variants2 = BTreeMap::new();
        variants2.insert(
            "python".to_string(),
            VariantValue::String("3.11".to_string()),
        );

        let source_data2 = CondaSourceData {
            package_record,
            location: UrlOrPath::from_str(".").unwrap(),
            variants: variants2,
            package_build_source: None,
            sources: BTreeMap::new(),
        };

        let id1 = SourceIdentifier::from_source_data(&source_data1);
        let id2 = SourceIdentifier::from_source_data(&source_data2);

        // Same name, but different hashes due to different variants
        assert_eq!(id1.name(), id2.name());
        assert_ne!(id1.hash(), id2.hash());
    }
}
