use crate::package::has_prefix::HasPrefixEntry;
use crate::package::{Files, HasPrefix, NoLink, NoSoftlink};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    str::FromStr,
};

/// A representation of the `paths.json` file found in package archives.
///
/// The `paths.json` file contains information about every file included with the package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathsJson {
    /// The version of the file
    pub paths_version: usize,

    /// All entries included in the package.
    pub paths: Vec<PathsEntry>,
}

impl PathsJson {
    /// Parses a `paths.json` file from a reader.
    pub fn from_reader(mut reader: impl Read) -> Result<Self, std::io::Error> {
        let mut str = String::new();
        reader.read_to_string(&mut str)?;
        Self::from_str(&str)
    }

    /// Parses a `paths.json` file from a file.
    pub fn from_path(path: &Path) -> Result<Self, std::io::Error> {
        Self::from_reader(File::open(path)?)
    }

    /// Reads the file from a package archive directory
    pub fn from_package_directory(path: &Path) -> Result<Self, std::io::Error> {
        Self::from_path(&path.join("info/paths.json"))
    }

    /// Reads the file from a package archive directory. If the `paths.json` file could not be found
    /// use the [`Self::from_deprecated_package_directory`] method as a fallback.
    pub fn from_package_directory_with_deprecated_fallback(
        path: &Path,
    ) -> Result<Self, std::io::Error> {
        match Self::from_package_directory(path) {
            Ok(paths) => Ok(paths),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Self::from_deprecated_package_directory(path)
            }
            Err(e) => Err(e),
        }
    }

    /// Constructs a new instance by looking at older (deprecated) files from a package directory.
    ///
    /// In older package archives the `paths.json` file does not exist. These packages contain the
    /// information normally present in the `paths.json` file spread over different files in the
    /// archive.
    ///
    /// This method takes parsed objects as input, to read the information from an extracted package
    /// use [`Self::from_deprecated_package_directory`].
    ///
    /// - The `files` file contains a list of all files included in the package.
    /// - The `has_prefix` file contains files that contain a "prefix".
    /// - The `no_link` file contains files that should not be linked.
    /// - The `no_softlink` file contains files that should not be soft-linked.
    /// - The `path_type` is a function to determine which type of file a specific path is.
    ///   Typically you would implement this with a function to check the filesystem.
    pub fn from_deprecated<E>(
        files: Files,
        has_prefix: Option<HasPrefix>,
        no_link: Option<NoLink>,
        no_softlink: Option<NoSoftlink>,
        path_type: impl Fn(&Path) -> Result<PathType, E>,
    ) -> Result<Self, E> {
        // Construct a HashSet of all paths that should not be linked.
        let no_link: HashSet<PathBuf> = {
            no_link
                .into_iter()
                .flat_map(|no_link| no_link.files.into_iter())
                .chain(
                    no_softlink
                        .into_iter()
                        .flat_map(|no_softlink| no_softlink.files.into_iter()),
                )
                .collect()
        };

        // Construct a mapping from path to prefix information
        let has_prefix: HashMap<PathBuf, HasPrefixEntry> = has_prefix
            .into_iter()
            .flat_map(|has_prefix| has_prefix.files.into_iter())
            .map(|entry| (entry.relative_path.clone(), entry))
            .collect();

        // Iterate over all files and create entries
        Ok(Self {
            paths: files
                .files
                .into_iter()
                .map(|path| {
                    let prefix = has_prefix.get(&path);
                    let path_type = path_type(&path);

                    match path_type {
                        Ok(path_type) => Ok(PathsEntry {
                            path_type,
                            file_mode: prefix
                                .map(|entry| entry.file_mode)
                                .unwrap_or(FileMode::Text),
                            prefix_placeholder: prefix
                                .map(|entry| entry.prefix.as_ref().to_owned()),
                            no_link: no_link.contains(&path),
                            sha256: None,
                            size_in_bytes: None,
                            relative_path: path,
                        }),
                        Err(e) => Err(e),
                    }
                })
                .collect::<Result<_, _>>()?,
            paths_version: 1,
        })
    }

    /// Constructs a new instance by reading older (deprecated) files from a package directory.
    ///
    /// In older package archives the `paths.json` file does not exist. These packages contain the
    /// information normally present in the `paths.json` file spread over different files in the
    /// archive.
    ///
    /// This function reads the different files and tries to reconstruct a `paths.json` from it.
    pub fn from_deprecated_package_directory(path: &Path) -> Result<Self, std::io::Error> {
        let files = Files::from_package_directory(path)?;

        let has_prefix = match HasPrefix::from_package_directory(path) {
            Ok(has_prefix) => Some(has_prefix),
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        let no_link = match NoLink::from_package_directory(path) {
            Ok(has_prefix) => Some(has_prefix),
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        let no_softlink = match NoSoftlink::from_package_directory(path) {
            Ok(has_prefix) => Some(has_prefix),
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };

        Self::from_deprecated(files, has_prefix, no_link, no_softlink, |p| {
            path.join(p).symlink_metadata().map(|metadata| {
                if metadata.is_symlink() {
                    PathType::SoftLink
                } else if metadata.is_dir() {
                    PathType::Directory
                } else {
                    PathType::HardLink
                }
            })
        })
    }
}

impl FromStr for PathsJson {
    type Err = std::io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s).map_err(Into::into)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct PathsEntry {
    /// The relative path from the root of the package
    #[serde(rename = "_path")]
    pub relative_path: PathBuf,

    /// Determines how to include the file when installing the package
    pub path_type: PathType,

    /// The type of the file, either binary or text.
    #[serde(default, skip_serializing_if = "FileMode::is_binary")]
    pub file_mode: FileMode,

    /// Optionally the placeholder prefix used in the file. If this value is `None` the prefix is not
    /// present in the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_placeholder: Option<String>,

    /// Whether or not this file should be linked or not when installing the package.
    #[serde(
        default = "no_link_default",
        skip_serializing_if = "is_no_link_default"
    )]
    pub no_link: bool,

    /// A hex representation of the SHA256 hash of the contents of the file.
    /// This entry is only present in version 1 of the paths.json file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,

    /// The size of the file in bytes
    /// This entry is only present in version 1 of the paths.json file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_in_bytes: Option<u64>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum FileMode {
    Binary,
    Text,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum PathType {
    HardLink,
    SoftLink,
    Directory,
}

impl Default for FileMode {
    fn default() -> Self {
        FileMode::Binary
    }
}

impl FileMode {
    /// Returns `true` if the file type is a binary file.
    pub fn is_binary(&self) -> bool {
        matches!(self, FileMode::Binary)
    }
}

/// Returns the default value for the "no_link" value of a [`PathsEntry`]
fn no_link_default() -> bool {
    false
}

/// Returns true if the value is equal to the default value for the "no_link" value of a [`PathsEntry`]
fn is_no_link_default(value: &bool) -> bool {
    *value == no_link_default()
}

#[cfg(test)]
mod test {
    use super::PathsJson;

    #[test]
    pub fn test_reconstruct_paths_json() {
        let package_dir = tempfile::tempdir().unwrap();
        rattler_package_streaming::fs::extract(
            &crate::get_test_data_dir().join("zlib-1.2.8-vc10_0.tar.bz2"),
            package_dir.path(),
        )
        .unwrap();

        insta::assert_yaml_snapshot!(PathsJson::from_deprecated_package_directory(
            package_dir.path()
        )
        .unwrap());
    }

    #[test]
    #[cfg(unix)]
    pub fn test_reconstruct_paths_json_with_symlinks() {
        let package_dir = tempfile::tempdir().unwrap();
        rattler_package_streaming::fs::extract(
            &crate::get_test_data_dir().join("with-symlinks/zlib-1.2.8-3.tar.bz2"),
            package_dir.path(),
        )
        .unwrap();

        let package_dir = package_dir.into_path();
        println!("{}", package_dir.display());

        insta::assert_yaml_snapshot!(
            PathsJson::from_deprecated_package_directory(&package_dir).unwrap()
        );
    }
}
