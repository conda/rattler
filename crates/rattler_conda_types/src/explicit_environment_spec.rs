//! An explicit environment file is Conda environment specification file that lists all of the
//! packages, dependencies and versions required to create a specific Conda environment. The file
//! can be used to recreate the environment on any other machine that has Conda installed, making it
//! a convenient and consistent way to manage dependencies for a project or application.
//!
//! Explicit environment files do not require a solver because they do not refer to package names
//! but instead directly refer to the download location of the package. This makes them useful
//! to quickly install an environment.
//!
//! To create an explicit environment file, you can use the `conda env export` command.

use crate::{ParsePlatformError, Platform};
use serde::{Deserialize, Serialize};
use std::{fs, fs::File, io::Read, path::Path, str::FromStr};
use url::Url;

/// An [`ExplicitEnvironmentSpec`] represents an explicit environment specification. Packages are
/// represented by a URL that defines from where they should be downloaded and they are already in
/// an explicit installation order. This ensures that there is no need to run the solver or to
/// download repodata which makes using explicit environments for installation of environments very
/// fast.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplicitEnvironmentSpec {
    /// Optionally the platform for which the environment can be created.
    ///
    /// This can be indicated by `# platform: <x>` in the environment file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<Platform>,

    /// Explicit package references
    pub packages: Vec<ExplicitEnvironmentEntry>,
}

/// A single entry in an [`ExplicitEnvironmentSpec`]. This is basically a representation of a package
/// URL. Package URLS can also have an associated URL hash which signifies the expected hash of
/// the package archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(into = "Url", from = "Url")]
pub struct ExplicitEnvironmentEntry {
    /// The url to download the package from
    pub url: Url,
}

/// Package urls in explicit environments can have an optional hash that signifies a hash of the
/// package archive. See [`ExplicitEnvironmentEntry::package_archive_hash`].
#[derive(Debug, Clone)]
pub enum PackageArchiveHash {
    /// An MD5 hash for a given package
    Md5(rattler_digest::Md5Hash),
    /// A SHA256 hash for a given package
    Sha256(rattler_digest::Sha256Hash),
}

/// An error that can occur when parsing a [`PackageArchiveHash`] from a string
#[derive(Debug, thiserror::Error)]
pub enum ParsePackageArchiveHashError {
    /// The hash is not a valid SHA256 hex string
    #[error("invalid sha256 hash")]
    InvalidSha256Hash(#[source] hex::FromHexError),

    /// The hash is not a valid MD5 hex string
    #[error("invalid md5 hash")]
    InvalidMd5Hash(#[source] hex::FromHexError),
}

impl FromStr for PackageArchiveHash {
    type Err = ParsePackageArchiveHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Parses a SHA256 hash from a string
        fn parse_sha256(str: &str) -> Result<PackageArchiveHash, ParsePackageArchiveHashError> {
            let mut hash = <rattler_digest::Sha256Hash>::default();
            hex::decode_to_slice(str, &mut hash)
                .map_err(ParsePackageArchiveHashError::InvalidSha256Hash)?;
            Ok(PackageArchiveHash::Sha256(hash))
        }

        fn parse_md5(str: &str) -> Result<PackageArchiveHash, ParsePackageArchiveHashError> {
            let mut hash = <rattler_digest::Md5Hash>::default();
            hex::decode_to_slice(str, &mut hash)
                .map_err(ParsePackageArchiveHashError::InvalidMd5Hash)?;
            Ok(PackageArchiveHash::Md5(hash))
        }

        if let Some(sha) = s.strip_prefix("sha256:") {
            // If the string starts with sha256 we parse as Sha256
            parse_sha256(sha)
        } else if s.len() == 64 {
            // If the string is 64 characters is length we parse as Sha256
            parse_sha256(s)
        } else {
            // Otherwise its an Md5
            parse_md5(s)
        }
    }
}

impl ExplicitEnvironmentEntry {
    /// If the url contains a hash section, that hash refers to the hash of the package archive.
    pub fn package_archive_hash(
        &self,
    ) -> Result<Option<PackageArchiveHash>, ParsePackageArchiveHashError> {
        self.url
            .fragment()
            .map_or(Ok(None), |s| PackageArchiveHash::from_str(s).map(Some))
    }
}

impl From<Url> for ExplicitEnvironmentEntry {
    fn from(url: Url) -> Self {
        ExplicitEnvironmentEntry { url }
    }
}

impl From<ExplicitEnvironmentEntry> for Url {
    fn from(entry: ExplicitEnvironmentEntry) -> Self {
        entry.url
    }
}

/// An error that can occur when parsing an [`ExplicitEnvironmentSpec`] from a string
#[derive(Debug, thiserror::Error)]
pub enum ParseExplicitEnvironmentSpecError {
    /// The @EXPLICIT tag is missing
    #[error("the @EXPLICIT tag is missing")]
    MissingExplicitTag,

    /// A invalid URL was present in the text file and could not be parsed
    #[error("failed to parse url '{0}'")]
    InvalidUrl(String, #[source] url::ParseError),

    /// The platform string could not be parsed
    #[error(transparent)]
    InvalidPlatform(#[from] ParsePlatformError),

    /// An IO error occurred
    #[error(transparent)]
    IoError(#[from] std::io::Error),
}

impl ExplicitEnvironmentSpec {
    /// Parses an explicit environment file from a reader.
    pub fn from_reader(mut reader: impl Read) -> Result<Self, ParseExplicitEnvironmentSpecError> {
        let mut str = String::new();
        reader.read_to_string(&mut str)?;
        Self::from_str(&str)
    }

    /// Parses an explicit environment file from a file.
    pub fn from_path(path: &Path) -> Result<Self, ParseExplicitEnvironmentSpecError> {
        Self::from_reader(File::open(path)?)
    }

    /// Converts an [`ExplicitEnvironmentSpec`] to a string representing a valid explicit
    /// environment file
    pub fn to_spec_string(&self) -> String {
        let mut s = String::new();

        if let Some(plat) = &self.platform {
            s.push_str(&format!("# platform: {plat}\n"));
        }

        s.push_str("@EXPLICIT\n");

        for p in &self.packages {
            s.push_str(&format!("{}\n", p.url.as_str()));
        }

        s
    }

    /// Writes an explicit environment spec to file
    pub fn to_path(&self, path: impl AsRef<Path>) -> Result<(), std::io::Error> {
        let s = self.to_spec_string();

        fs::write(path, s)?;

        Ok(())
    }
}

impl FromStr for ExplicitEnvironmentSpec {
    type Err = ParseExplicitEnvironmentSpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut platform = None;
        let mut is_explicit = false;
        let mut packages = Vec::new();
        for line in s.lines() {
            // Skip lines starting with a #
            if let Some(comment_line) = line.strip_prefix('#') {
                // Unless that comment line is `# platform: `. Because then we're interested in the
                // platform specifier.
                if let Some(platform_str) = comment_line.trim_start().strip_prefix("platform:") {
                    platform = Some(Platform::from_str(platform_str.trim())?);
                }
            } else if line.trim() == "@EXPLICIT" {
                is_explicit = true;
            } else if !is_explicit {
                return Err(ParseExplicitEnvironmentSpecError::MissingExplicitTag);
            } else {
                // Parse the line as an explicit URL
                packages.push(
                    Url::parse(line.trim())
                        .map_err(|e| {
                            ParseExplicitEnvironmentSpecError::InvalidUrl(line.trim().to_owned(), e)
                        })?
                        .into(),
                );
            }
        }

        if !is_explicit {
            return Err(ParseExplicitEnvironmentSpecError::MissingExplicitTag);
        }

        Ok(ExplicitEnvironmentSpec { platform, packages })
    }
}

#[cfg(test)]
mod test {
    use super::{ExplicitEnvironmentSpec, ParseExplicitEnvironmentSpecError};
    use crate::{
        explicit_environment_spec::{PackageArchiveHash, ParsePackageArchiveHashError},
        get_test_data_dir, ExplicitEnvironmentEntry,
    };
    use assert_matches::assert_matches;
    use hex_literal::hex;
    use rstest::rstest;
    use std::str::FromStr;
    use url::Url;

    #[rstest]
    #[case::ros_noetic_linux_64("explicit-envs/ros-noetic_linux-64.txt")]
    #[case::vs2015_runtime_win_64("explicit-envs/vs2015_runtime_win-64.txt")]
    #[case::xtensor_linux_64("explicit-envs/xtensor_linux-64.txt")]
    fn test_parse(#[case] path: &str) {
        let env = ExplicitEnvironmentSpec::from_path(&get_test_data_dir().join(path)).unwrap();
        insta::assert_yaml_snapshot!(path, env);
    }

    #[test]
    fn test_parse_empty() {
        assert_matches!(
            ExplicitEnvironmentSpec::from_str(""),
            Err(ParseExplicitEnvironmentSpecError::MissingExplicitTag)
        );
    }

    #[test]
    fn test_parse_no_explicit_tag() {
        assert_matches!(
            ExplicitEnvironmentSpec::from_str("https://repo.anaconda.com/pkgs/main/win-64/vs2015_runtime-14.16.27012-hf0eaf9b_3.conda#a98ea1e3abfdbbd201d60ff6b43ea7e4"),
            Err(ParseExplicitEnvironmentSpecError::MissingExplicitTag)
        );
    }

    #[test]
    fn test_parse_invalid_url() {
        assert_matches!(
            ExplicitEnvironmentSpec::from_str("@EXPLICIT\nimnotanurl"),
            Err(ParseExplicitEnvironmentSpecError::InvalidUrl(url, _)) if url == "imnotanurl"
        );
    }

    #[test]
    fn test_parse_invalid_platform() {
        assert_matches!(
            ExplicitEnvironmentSpec::from_str("# platform: notaplatform\n@EXPLICIT"),
            Err(ParseExplicitEnvironmentSpecError::InvalidPlatform(_))
        );
    }

    #[rstest]
    #[case::ros_noetic_linux_64("explicit-envs/ros-noetic_linux-64.txt")]
    #[case::vs2015_runtime_win_64("explicit-envs/vs2015_runtime_win-64.txt")]
    #[case::xtensor_linux_64("explicit-envs/xtensor_linux-64.txt")]
    fn test_to_spec_string(#[case] path: &str) {
        let env = ExplicitEnvironmentSpec::from_path(&get_test_data_dir().join(path)).unwrap();
        let env_cmp = ExplicitEnvironmentSpec::from_str(&env.to_spec_string()).unwrap();

        assert_eq!(env.platform, env_cmp.platform);
        assert_eq!(
            env.packages
                .iter()
                .map(|entry| entry.url.clone())
                .collect::<Vec<_>>(),
            env_cmp
                .packages
                .iter()
                .map(|entry| entry.url.clone())
                .collect::<Vec<_>>()
        );
    }

    #[rstest]
    #[case::ros_noetic_linux_64("explicit-envs/ros-noetic_linux-64.txt")]
    #[case::vs2015_runtime_win_64("explicit-envs/vs2015_runtime_win-64.txt")]
    #[case::xtensor_linux_64("explicit-envs/xtensor_linux-64.txt")]
    fn test_to_path(#[case] path: &str) {
        let env = ExplicitEnvironmentSpec::from_path(&get_test_data_dir().join(path)).unwrap();

        let tmp_dir = tempfile::tempdir().unwrap();
        let file_path = tmp_dir.path().join("temp_explicit_env.txt");

        assert_matches!(env.to_path(file_path.clone()), Ok(()));

        // Check file content round trip
        let round_trip_env = ExplicitEnvironmentSpec::from_path(&file_path).unwrap();

        assert_eq!(env.platform, round_trip_env.platform);
        assert_eq!(
            env.packages
                .iter()
                .map(|entry| entry.url.clone())
                .collect::<Vec<_>>(),
            round_trip_env
                .packages
                .iter()
                .map(|entry| entry.url.clone())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_entry_package_hash() {
        let entry: ExplicitEnvironmentEntry = Url::parse("https://repo.anaconda.com/pkgs/main/win-64/vs2015_runtime-14.16.27012-hf0eaf9b_3.conda#a98ea1e3abfdbbd201d60ff6b43ea7e4").unwrap().into();
        assert_matches!(
            entry.package_archive_hash(),
            Ok(Some(PackageArchiveHash::Md5(hash))) if hash[..] == hex!("a98ea1e3abfdbbd201d60ff6b43ea7e4")
        );
    }

    #[test]
    fn test_parse_entry_hash() {
        // Parse empty
        assert_matches!(
            PackageArchiveHash::from_str(""),
            Err(ParsePackageArchiveHashError::InvalidMd5Hash(_))
        );

        // Parse regular md5
        assert_matches!(
            PackageArchiveHash::from_str("a98ea1e3abfdbbd201d60ff6b43ea7e4"),
            Ok(PackageArchiveHash::Md5(hash)) if hash[..] == hex!("a98ea1e3abfdbbd201d60ff6b43ea7e4")
        );
        assert_matches!(
            PackageArchiveHash::from_str("dc9507a39ab328597820486c729c"),
            Err(ParsePackageArchiveHashError::InvalidMd5Hash(_))
        );

        // Parse based on tag
        assert_matches!(
            PackageArchiveHash::from_str("sha256:blablabla"),
            Err(ParsePackageArchiveHashError::InvalidSha256Hash(_))
        );
        assert_matches!(
            PackageArchiveHash::from_str(
                "sha256:315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3"
            ),
            Ok(PackageArchiveHash::Sha256(hash)) if hash[..] == hex!("315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3")
        );

        // Parse based on length (64 characters is sha256 hash)
        assert_matches!(
            PackageArchiveHash::from_str(
                "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3"
            ),
            Ok(PackageArchiveHash::Sha256(hash)) if hash[..] == hex!("315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3")
        );
        assert_matches!(
            PackageArchiveHash::from_str(
                "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c7589"
            ),
            Err(ParsePackageArchiveHashError::InvalidMd5Hash(_))
        );
    }
}
