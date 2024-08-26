use super::PackageFile;
use crate::{
    package::has_prefix::HasPrefixEntry,
    package::{Files, HasPrefix, NoLink, NoSoftlink},
};
use rattler_digest::serde::SerializableHash;
use rattler_macros::sorted;
use serde::{Deserialize, Serialize, Serializer};
use serde_with::serde_as;
use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// A representation of the `paths.json` file found in package archives.
///
/// The `paths.json` file contains information about every file included with the package.
#[sorted]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathsJson {
    /// All entries included in the package.
    #[serde(serialize_with = "serialize_sorted_paths")]
    pub paths: Vec<PathsEntry>,

    /// The version of the file
    pub paths_version: u64,
}

fn serialize_sorted_paths<S>(paths: &[PathsEntry], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    // Sort the paths by the relative_path attribute
    let mut sorted_paths = paths.to_vec();
    sorted_paths.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    sorted_paths.serialize(serializer)
}

impl PackageFile for PathsJson {
    fn package_path() -> &'static Path {
        Path::new("info/paths.json")
    }

    fn from_str(str: &str) -> Result<Self, std::io::Error> {
        serde_json::from_str(str).map_err(Into::into)
    }
}

impl PathsJson {
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
                            prefix_placeholder: prefix.map(|entry| PrefixPlaceholder {
                                file_mode: entry.file_mode,
                                placeholder: (*entry.prefix).to_owned(),
                            }),
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

/// Description off a placeholder text found in a file that must be replaced when installing the
/// file into the prefix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct PrefixPlaceholder {
    /// The type of the file, either binary or text. Depending on the type of file either text
    /// replacement is performed or `CString` replacement.
    pub file_mode: FileMode,

    /// The placeholder prefix used in the file. This is the path of the prefix when the package
    /// was build.
    #[serde(rename = "prefix_placeholder")]
    pub placeholder: String,
}

/// A single entry in the `paths.json` file.
#[serde_as]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PathsEntry {
    // The alphabetical order of the fields is important for the serialization of the struct.
    // ['_path', 'no_link', 'path_type', 'prefix_placeholder', 'sha256', 'size_in_bytes']
    // rename can't be sorted by the macro yet.
    /// The relative path from the root of the package
    #[serde(rename = "_path")]
    #[serde_as(as = "crate::utils::serde::NormalizedPath")]
    pub relative_path: PathBuf,

    /// Whether or not this file should be linked or not when installing the package.
    #[serde(
        default = "no_link_default",
        skip_serializing_if = "is_no_link_default"
    )]
    pub no_link: bool,

    /// Determines how to include the file when installing the package
    pub path_type: PathType,

    /// Optionally the placeholder prefix used in the file. If this value is `None` the prefix is not
    /// present in the file.
    #[serde(default, flatten, skip_serializing_if = "Option::is_none")]
    pub prefix_placeholder: Option<PrefixPlaceholder>,

    /// A hex representation of the SHA256 hash of the contents of the file.
    /// This entry is only present in version 1 of the paths.json file.
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Sha256>>")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<rattler_digest::Sha256Hash>,

    /// The size of the file in bytes
    /// This entry is only present in version 1 of the paths.json file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_in_bytes: Option<u64>,
}

/// The file mode of the entry
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum FileMode {
    /// The file is a binary file (needs binary prefix replacement)
    Binary,
    /// The file is a text file (needs text prefix replacement)
    Text,
}

/// The path type of the path entry
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum PathType {
    /// The path should be hard linked (the default)
    HardLink,
    /// The path should be soft linked
    SoftLink,
    /// This should explicitly create an empty directory
    Directory,
}

/// Returns the default value for the `no_link` value of a [`PathsEntry`]
fn no_link_default() -> bool {
    false
}

/// Returns true if the value is equal to the default value for the `no_link` value of a [`PathsEntry`]
fn is_no_link_default(value: &bool) -> bool {
    *value == no_link_default()
}

#[cfg(test)]
mod test {
    use crate::package::PackageFile;

    use super::{PathsEntry, PathsJson};

    #[test]
    pub fn roundtrip_paths_json() {
        // TODO make sure that paths.json is sorted by `_path`!
        let package_dir = tempfile::tempdir().unwrap();
        let package_path = tools::download_and_cache_file(
            "https://conda.anaconda.org/conda-forge/win-64/mamba-1.0.0-py38hecfeebb_2.tar.bz2"
                .parse()
                .unwrap(),
            "f44c4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97",
        )
        .unwrap();
        rattler_package_streaming::fs::extract(&package_path, package_dir.path()).unwrap();

        let paths_json = PathsJson::from_package_directory(package_dir.path()).unwrap();
        insta::assert_yaml_snapshot!(paths_json);
    }

    #[test]
    pub fn test_reconstruct_paths_json() {
        let package_dir = tempfile::tempdir().unwrap();
        let package_path = tools::download_and_cache_file(
            "https://conda.anaconda.org/conda-forge/win-64/zlib-1.2.8-vc10_0.tar.bz2"
                .parse()
                .unwrap(),
            "ee9172dbe9ebd158e8e68d6d0f7dc2060f0c8230b44d2e9a3595b7cd7336b915",
        )
        .unwrap();
        rattler_package_streaming::fs::extract(&package_path, package_dir.path()).unwrap();

        insta::assert_yaml_snapshot!(PathsJson::from_deprecated_package_directory(
            package_dir.path()
        )
        .unwrap());
    }

    #[test]
    #[cfg(unix)]
    pub fn test_reconstruct_paths_json_with_symlinks() {
        let package_dir = tempfile::tempdir().unwrap();

        let package_path = tools::download_and_cache_file(
            "https://conda.anaconda.org/conda-forge/linux-64/zlib-1.2.8-3.tar.bz2"
                .parse()
                .unwrap(),
            "85fcb6906b8686fe6341db89b4e6fc2631ad69ee6eab2f4823bfd64ae0b20ac8",
        )
        .unwrap();
        rattler_package_streaming::fs::extract(&package_path, package_dir.path()).unwrap();

        let package_dir = package_dir.into_path();
        println!("{}", package_dir.display());

        insta::assert_yaml_snapshot!(
            PathsJson::from_deprecated_package_directory(&package_dir).unwrap()
        );
    }

    #[test]
    pub fn test_paths_sorted() {
        use rand::seq::SliceRandom;

        // create some fake data
        let mut paths = vec![];
        for i in 0..15 {
            paths.push(PathsEntry {
                relative_path: format!("rel\\path_{i}").into(),
                path_type: super::PathType::HardLink,
                prefix_placeholder: None,
                no_link: false,
                sha256: None,
                size_in_bytes: Some(0),
            });
        }

        // shuffle the data
        let mut rng = rand::thread_rng();
        paths.shuffle(&mut rng);

        insta::assert_yaml_snapshot!(PathsJson {
            paths,
            paths_version: 1
        });
    }
}
