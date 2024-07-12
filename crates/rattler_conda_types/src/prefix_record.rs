//! Defines the `[PrefixRecord]` struct.

use crate::package::FileMode;
use crate::repo_data_record::RepoDataRecord;
use crate::PackageRecord;
use rattler_digest::serde::SerializableHash;
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use serde_with::serde_as;
use std::fs::File;
use std::io::{BufWriter, Read};
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Information about every file installed with the package.
///
/// This struct is similar to the [`crate::package::PathsJson`] struct. The difference is that this
/// information refers to installed files whereas [`crate::package::PathsJson`] describes the
/// instructions on how to install a package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrefixPaths {
    /// The version of the file
    pub paths_version: u64,

    /// All entries included in the package.
    #[serde(default)]
    pub paths: Vec<PathsEntry>,
}

impl Default for PrefixPaths {
    fn default() -> Self {
        Self {
            paths_version: 1,
            paths: Vec::default(),
        }
    }
}

impl From<Vec<PathsEntry>> for PrefixPaths {
    fn from(paths: Vec<PathsEntry>) -> Self {
        Self {
            paths,
            ..Default::default()
        }
    }
}

/// Information about a single file installed for a package.
///
/// This struct is similar to the [`crate::package::PathsEntry`] struct. The difference is that this
/// information refers to installed files whereas [`crate::package::PathsEntry`] describes the
/// instructions on how to install a file.
#[serde_as]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct PathsEntry {
    /// The relative path from the root of the package
    #[serde(rename = "_path")]
    #[serde_as(as = "crate::utils::serde::NormalizedPath")]
    pub relative_path: PathBuf,

    /// The original path of the file in the package. This is only set if the file was clobbered by
    /// another package and therefore the original path is not the same as the relative path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_path: Option<PathBuf>,

    /// How the file was installed into the environment
    pub path_type: PathType,

    /// Whether or not this file should not be linked
    #[serde(
        default = "no_link_default",
        skip_serializing_if = "is_no_link_default"
    )]
    pub no_link: bool,

    /// A hex representation of the SHA256 hash of the contents of the file.
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Sha256>>")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<rattler_digest::Sha256Hash>,

    /// A hex representation of the SHA256 hash of the original file from which this was created.
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Sha256>>")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256_in_prefix: Option<rattler_digest::Sha256Hash>,

    /// The size of the file in bytes
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_in_bytes: Option<u64>,

    /// The file mode of the entry. This is used in conjunction with a prefix_placeholder
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_mode: Option<FileMode>,

    /// The original sentinel value used for prefix-replacement from the package
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_placeholder: Option<String>,
}

/// Information about a single file installed for a package.
///
/// This enum is similar to the [`crate::package::PathType`] enum. This enum includes more enum entries
/// that are created when a file is installed.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PathType {
    /// The file was installed as a hard link (i.e. a file that points to another file in the package cache)
    #[serde(rename = "hardlink")]
    HardLink,
    #[serde(rename = "softlink")]
    /// The file was installed as a soft link (i.e. a soft link to a file in the package cache)
    SoftLink,
    /// An empty directory was created at installation time here
    Directory,
    /// This is a file that was automatically "compiled" from Python source code when a `noarch` package
    /// was installed
    PycFile,
    /// This file is a Python entry point script for Windows (a `<entrypoint>-script.py` Python script file)
    WindowsPythonEntryPointScript,
    /// This file is a Python entry point executable for Windows (a `<entrypoint>.exe` file)
    WindowsPythonEntryPointExe,
    /// This file is a Python entry point executable for Unix (a `<entrypoint>` Python script file)
    /// Entry points are created in the `bin/...` directory when installing Python noarch packages
    UnixPythonEntryPoint,
    /// NOT USED - path to the package's .json file in conda-meta
    LinkedPackageRecord,
}

impl From<crate::package::PathType> for PathType {
    fn from(ty: crate::package::PathType) -> Self {
        match ty {
            crate::package::PathType::HardLink => PathType::HardLink,
            crate::package::PathType::SoftLink => PathType::SoftLink,
            crate::package::PathType::Directory => PathType::Directory,
        }
    }
}

/// A record of a single package installed within an environment. The struct includes the
/// [`RepoDataRecord`] which specifies information about where the original package comes from.
#[serde_as]
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone)]
pub struct PrefixRecord {
    /// The data from the repodata
    #[serde(flatten)]
    pub repodata_record: RepoDataRecord,

    /// The path to where the archive of the package was stored on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_tarball_full_path: Option<PathBuf>,

    /// The path that contains the extracted package content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extracted_package_dir: Option<PathBuf>,

    /// A sorted list of all files included in this package
    #[serde(default)]
    #[serde_as(as = "Vec<crate::utils::serde::NormalizedPath>")]
    pub files: Vec<PathBuf>,

    /// Information about how files have been linked when installing the package.
    #[serde(default)]
    pub paths_data: PrefixPaths,

    /// This field contains a reference to the package cache from where the package was linked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<Link>,

    /// The spec that was used when this package was installed. Note that this field is not updated if the
    /// currently another spec was used. Note: conda seems to serialize a "None" string value instead of `null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_spec: Option<String>,
}

impl PrefixRecord {
    /// Parses a `paths.json` file from a reader.
    pub fn from_reader(mut reader: impl Read) -> Result<Self, std::io::Error> {
        let mut str = String::new();
        reader.read_to_string(&mut str)?;
        simd_json::serde::from_slice(&mut str.into_bytes()).map_err(Into::into)
    }

    /// Creates a `PrefixRecord` from a `RepoDataRecord`.
    pub fn from_repodata_record(
        repodata_record: RepoDataRecord,
        package_tarball_full_path: Option<PathBuf>,
        extracted_package_dir: Option<PathBuf>,
        paths: Vec<PathsEntry>,
        requested_spec: Option<String>,
        link: Option<Link>,
    ) -> Self {
        Self {
            repodata_record,
            package_tarball_full_path,
            extracted_package_dir,
            files: paths
                .iter()
                .map(|entry| entry.relative_path.clone())
                .collect(),
            paths_data: paths.into(),
            link,
            requested_spec,
        }
    }

    /// Parses a `paths.json` file from a file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        Self::from_reader(File::open(path.as_ref())?)
    }

    /// Return the canonical file name for a `PrefixRecord`. Takes the form of
    /// `<package_name>-<version>-<build>.json`.
    pub fn file_name(&self) -> String {
        format!(
            "{}-{}-{}.json",
            self.repodata_record.package_record.name.as_normalized(),
            self.repodata_record.package_record.version,
            self.repodata_record.package_record.build
        )
    }

    /// Writes the contents of this instance to the file at the specified location.
    pub fn write_to_path(
        &self,
        path: impl AsRef<Path>,
        pretty: bool,
    ) -> Result<(), std::io::Error> {
        self.write_to(File::create(path)?, pretty)
    }

    /// Writes the contents of this instance to the file at the specified location.
    pub fn write_to(
        &self,
        writer: impl std::io::Write,
        pretty: bool,
    ) -> Result<(), std::io::Error> {
        if pretty {
            serde_json::to_writer_pretty(BufWriter::new(writer), self)?;
        } else {
            serde_json::to_writer(BufWriter::new(writer), self)?;
        }
        Ok(())
    }

    /// Collects all `PrefixRecord`s from the specified prefix. This function will read all files in
    /// the `$PREFIX/conda-meta` directory and parse them as `PrefixRecord`s.
    pub fn collect_from_prefix(prefix: &Path) -> Result<Vec<PrefixRecord>, std::io::Error> {
        let mut records = Vec::new();
        let conda_meta_path = prefix.join("conda-meta");

        if !conda_meta_path.exists() {
            return Ok(records);
        }

        for entry in std::fs::read_dir(prefix.join("conda-meta"))? {
            let entry = entry?;

            if entry.file_type()?.is_file()
                && entry.file_name().to_string_lossy().ends_with(".json")
            {
                let record = Self::from_path(entry.path())?;
                records.push(record);
            }
        }
        Ok(records)
    }
}

impl FromStr for PrefixRecord {
    type Err = std::io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s).map_err(Into::into)
    }
}

/// A record of a single file that was installed into the prefix
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone)]
pub struct Link {
    /// The path to the file source that was installed
    pub source: PathBuf,

    /// The link type that was used to install the file
    #[serde(rename = "type")]
    pub link_type: Option<LinkType>,
}

/// The different link types that are used
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize_repr, Deserialize_repr, Hash)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum LinkType {
    /// Hard link refers to the same inode as the source file
    HardLink = 1,
    /// Soft link is a soft link to the source file
    SoftLink = 2,
    /// Copy is a proper copy of the source file (duplicated data on hard disk)
    Copy = 3,
    /// Directory is a (empty) directory
    Directory = 4,
}

/// Returns the default value for the `no_link` value of a [`PathsEntry`]
fn no_link_default() -> bool {
    false
}

/// Returns true if the value is equal to the default value for the `no_link` value of a [`PathsEntry`]
fn is_no_link_default(value: &bool) -> bool {
    *value == no_link_default()
}

impl AsRef<RepoDataRecord> for PrefixRecord {
    fn as_ref(&self) -> &RepoDataRecord {
        &self.repodata_record
    }
}

impl AsRef<PackageRecord> for PrefixRecord {
    fn as_ref(&self) -> &PackageRecord {
        &self.repodata_record.package_record
    }
}

#[cfg(test)]
mod test {
    use crate::get_test_data_dir;
    use rstest::rstest;

    #[rstest]
    #[case::xz_5_2_6_h8d14728_0("xz-5.2.6-h8d14728_0.json")]
    #[case::libsqlite_3_40_0_hcfcfb64_0("libsqlite-3.40.0-hcfcfb64_0.json")]
    #[case::menuinst_1_4_19_py311h1ea47a8_1("menuinst-1.4.19-py311h1ea47a8_1.json")]
    #[case::pip_23_0_pyhd8ed1ab_0_json("pip-23.0-pyhd8ed1ab_0.json")]
    #[case::pysocks_1_7_1_pyh0701188_6("pysocks-1.7.1-pyh0701188_6.json")]
    #[case::requests_2_28_2_pyhd8ed1ab_0("requests-2.28.2-pyhd8ed1ab_0.json")]
    #[case::tk_8_6_12_h8ffe710_0("tk-8.6.12-h8ffe710_0.json")]
    #[case::urllib3_1_26_14_pyhd8ed1ab_0("urllib3-1.26.14-pyhd8ed1ab_0.json")]
    #[case::vc_14_3_hb6edc58_10_json("vc-14.3-hb6edc58_10.json")]
    #[case::wheel_0_38_4_pyhd8ed1ab_0("wheel-0.38.4-pyhd8ed1ab_0.json")]
    fn parse_prefix_record(#[case] path_name: &str) {
        let path = get_test_data_dir().join("conda-meta").join(path_name);
        let prefix_record = super::PrefixRecord::from_path(path).unwrap();
        insta::assert_yaml_snapshot!(path_name.replace('.', "_"), prefix_record);
    }
}
