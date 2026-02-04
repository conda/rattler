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
                                offsets: None,
                            }),
                            no_link: no_link.contains(&path),
                            sha256: None,
                            size_in_bytes: None,
                            relative_path: path,
                            executable: None,
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

    /// The offsets on which the placeholders are found in the file
    /// only present in version 2 of the paths.json file
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offsets: Option<Vec<usize>>,
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
    /// This entry is present in version 1 and up of the paths.json file.
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Sha256>>")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<rattler_digest::Sha256Hash>,

    /// The size of the file in bytes
    /// This entry is present in version 1 and up of the paths.json file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_in_bytes: Option<u64>,

    /// When a file is executable this will be true
    /// This entry is only present in version 2 of the paths.json file
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<bool>,
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
    use crate::package::{PackageFile, PrefixPlaceholder};

    use super::{FileMode, PathBuf, PathType, PathsEntry, PathsJson};

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

        let package_dir = package_dir.keep();
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
                executable: None,
            });
        }

        // shuffle the data
        let mut rng = rand::rng();
        paths.shuffle(&mut rng);

        insta::assert_yaml_snapshot!(PathsJson {
            paths,
            paths_version: 1
        });
    }

    #[test]
    pub fn test_deserialize_paths_json_v2() {
        let package_dir = tempfile::tempdir().unwrap();
        let info_dir = package_dir.path().join("info");
        std::fs::create_dir_all(&info_dir).unwrap();

        // Create a mock v2 paths.json file
        let paths_json_v2 = r#"{
            "paths": [
                {
                    "_path": "bin/example",
                    "no_link": false,
                    "path_type": "hardlink",
                    "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    "size_in_bytes": 1024,
                    "executable": true,
                    "file_mode": "binary",
                    "prefix_placeholder": "/opt/conda",
                    "offsets": [100, 200, 300]
                },
                {
                    "_path": "lib/library.so",
                    "no_link": false,
                    "path_type": "hardlink",
                    "sha256": "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592",
                    "size_in_bytes": 2048,
                    "executable": false
                },
                {
                    "_path": "share/doc/readme.txt",
                    "no_link": false,
                    "path_type": "hardlink",
                    "sha256": "a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3",
                    "size_in_bytes": 256,
                    "executable": false,
                    "file_mode": "text",
                    "prefix_placeholder": "/home/builder/conda",
                    "offsets": [10, 45]
                },
                {
                    "_path": "bin/symlink-example",
                    "no_link": false,
                    "path_type": "softlink",
                    "executable": true
                }
            ],
            "paths_version": 2
            }"#;

        // Write the mock paths.json
        std::fs::write(info_dir.join("paths.json"), paths_json_v2).unwrap();

        // Test loading it
        let paths_json =
            PathsJson::from_package_directory_with_deprecated_fallback(package_dir.path()).unwrap();

        // Verify it's version 2
        assert_eq!(paths_json.paths_version, 2);
        assert_eq!(paths_json.paths.len(), 4);

        // Verify v2-specific fields are present and correct

        // First entry: binary with offsets and executable
        assert_eq!(
            paths_json.paths[0].relative_path,
            PathBuf::from("bin/example")
        );
        assert_eq!(paths_json.paths[0].executable, Some(true));
        assert_eq!(paths_json.paths[0].size_in_bytes, Some(1024));
        let prefix = paths_json.paths[0].prefix_placeholder.as_ref().unwrap();
        assert_eq!(prefix.file_mode, FileMode::Binary);
        assert_eq!(prefix.offsets, Some(vec![100, 200, 300]));

        // Second entry: no prefix, not executable
        assert_eq!(paths_json.paths[1].executable, Some(false));
        assert!(paths_json.paths[1].prefix_placeholder.is_none());

        // Third entry: text with offsets
        let text_prefix = paths_json.paths[2].prefix_placeholder.as_ref().unwrap();
        assert_eq!(text_prefix.file_mode, FileMode::Text);
        assert_eq!(text_prefix.offsets, Some(vec![10, 45]));

        // Fourth entry: symlink with executable
        assert_eq!(paths_json.paths[3].path_type, PathType::SoftLink);
        assert_eq!(paths_json.paths[3].executable, Some(true));

        insta::assert_yaml_snapshot!(paths_json);
    }

    #[test]
    pub fn test_v2_optional_fields_handling() {
        let package_dir = tempfile::tempdir().unwrap();
        let info_dir = package_dir.path().join("info");
        std::fs::create_dir_all(&info_dir).unwrap();

        // Test that v2 fields are truly optional
        let minimal_v2 = r#"{
            "paths": [
                {
                "_path": "file.txt",
                "path_type": "hardlink"
                }
            ],
            "paths_version": 2
            }"#;

        std::fs::write(info_dir.join("paths.json"), minimal_v2).unwrap();

        let paths_json = PathsJson::from_package_directory(package_dir.path()).unwrap();

        assert_eq!(paths_json.paths_version, 2);
        assert_eq!(paths_json.paths[0].executable, None);
        assert_eq!(paths_json.paths[0].sha256, None);
        assert_eq!(paths_json.paths[0].size_in_bytes, None);
        assert!(paths_json.paths[0].prefix_placeholder.is_none());
    }

    #[test]
    pub fn test_v2_serialization_roundtrip() {
        // Create a v2 PathsJson programmatically
        let original = PathsJson {
            paths: vec![
                PathsEntry {
                    relative_path: PathBuf::from("bin/tool"),
                    no_link: false,
                    path_type: PathType::HardLink,
                    prefix_placeholder: Some(PrefixPlaceholder {
                        file_mode: FileMode::Binary,
                        placeholder: "/opt/conda".to_string(),
                        offsets: Some(vec![50, 150]),
                    }),
                    sha256: None,
                    size_in_bytes: Some(4096),
                    executable: Some(true),
                },
                PathsEntry {
                    relative_path: PathBuf::from("lib/module.py"),
                    no_link: false,
                    path_type: PathType::HardLink,
                    prefix_placeholder: None,
                    sha256: None,
                    size_in_bytes: Some(512),
                    executable: Some(false),
                },
            ],
            paths_version: 2,
        };

        // Serialize to JSON
        let json = serde_json::to_string_pretty(&original).unwrap();

        // Deserialize back
        let deserialized: PathsJson = serde_json::from_str(&json).unwrap();

        // Verify roundtrip
        assert_eq!(original, deserialized);
        assert_eq!(deserialized.paths_version, 2);
        assert_eq!(deserialized.paths[0].executable, Some(true));
        assert_eq!(
            deserialized.paths[0]
                .prefix_placeholder
                .as_ref()
                .unwrap()
                .offsets,
            Some(vec![50, 150])
        );
    }

    #[test]
    pub fn test_fallback_from_v2_to_deprecated() {
        let package_dir = tempfile::tempdir().unwrap();
        let info_dir = package_dir.path().join("info");
        std::fs::create_dir_all(&info_dir).unwrap();

        // Don't create paths.json, but create deprecated files
        let files_content = "bin/old-tool\nlib/old-lib.so\n";
        std::fs::write(info_dir.join("files"), files_content).unwrap();

        // Create actual files so path_type detection works
        let bin_dir = package_dir.path().join("bin");
        let lib_dir = package_dir.path().join("lib");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::create_dir_all(&lib_dir).unwrap();
        std::fs::write(bin_dir.join("old-tool"), "#!/bin/sh\necho test").unwrap();
        std::fs::write(lib_dir.join("old-lib.so"), "binary data").unwrap();

        let paths_json =
            PathsJson::from_package_directory_with_deprecated_fallback(package_dir.path()).unwrap();

        // Should fall back and create v1
        assert_eq!(paths_json.paths_version, 1);
        assert_eq!(paths_json.paths.len(), 2);

        // v1 shouldn't have v2 fields
        assert!(paths_json.paths.iter().all(|p| p.executable.is_none()));
    }
}
