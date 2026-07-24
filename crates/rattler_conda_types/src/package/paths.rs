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

    fn from_slice(slice: &[u8]) -> Result<Self, std::io::Error> {
        serde_json::from_slice(slice).map_err(Into::into)
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
                                experimental_offsets: None,
                                experimental_shebang_length: None,
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

    /// The placeholder's occurrences in the file, recorded per encoding.
    ///
    /// Each [`OffsetGroup`] lists the byte positions at which the placeholder
    /// occurs in the file contents as stored in the package (before any
    /// replacement) under one encoding. Producers search all five defined
    /// encodings before emitting the field; the absence of a group is a
    /// statement that the file contains no occurrences under that encoding.
    /// Installers apply exactly the groups whose encodings their own
    /// search-based replacement covers — rattler's covers UTF-8 only, see
    /// [`select_utf8_offset_ranges`].
    ///
    /// Occurrences inside the shebang region (the first
    /// [`Self::experimental_shebang_length`] bytes) are **excluded** — that region is
    /// transformed by the installer's shebang rules rather than by plain
    /// splicing at recorded offsets — so every recorded value is greater than
    /// or equal to `shebang_length` when it is present. The list must not
    /// contain two groups with the same encoding, the ranges recorded across
    /// all groups must not overlap, and groups SHOULD be serialized sorted by
    /// encoding so equivalent metadata serializes identically.
    ///
    /// `None` for older packages or packages whose publisher did not
    /// populate the field — callers must scan the file themselves in
    /// that case. A value that does not parse as offset groups (for example
    /// the flat lists written by pre-CEP drafts of this field) is also
    /// treated as absent rather than failing the whole `paths.json`.
    /// **Experimental**: the Rust field is prefixed until
    /// [conda/ceps#179](https://github.com/conda/ceps/pull/179) is finalized
    /// and may change or disappear; the serialized form is `offsets`.
    #[serde(
        rename = "offsets",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_offset_groups"
    )]
    pub experimental_offsets: Option<Vec<OffsetGroup>>,

    /// The length in bytes of the file's shebang region: the first line
    /// including its terminating newline, or the whole file size when the
    /// file contains no newline.
    ///
    /// Present if and only if [`Self::experimental_offsets`] is present, `file_mode` is
    /// [`FileMode::Text`], and the file starts with the bytes `#!` —
    /// regardless of whether the first line itself contains the placeholder,
    /// because installers collapse an over-long shebang even when it does not.
    ///
    /// Lets consumers compute the exact post-replacement file size and locate
    /// the region an installer transforms under its shebang rules without
    /// rendering the whole file. `None` for binary-mode placeholders, text
    /// files that do not start with a shebang, or older packages.
    /// **Experimental**: the Rust field is prefixed until
    /// [conda/ceps#179](https://github.com/conda/ceps/pull/179) is finalized
    /// and may change or disappear; the serialized form is `shebang_length`.
    #[serde(
        rename = "shebang_length",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub experimental_shebang_length: Option<usize>,
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
}

/// The encoding of one [`OffsetGroup`].
///
/// The closed set of names mirrors the encodings replaced by existing
/// installers: conda searches UTF-8 plus the UTF-16 and UTF-32 variants,
/// while rattler and libmamba search UTF-8 only. A name outside this set
/// deserializes as [`OffsetEncoding::Unknown`] rather than failing the whole
/// `paths.json`: per the CEP an unknown encoding is indistinguishable from
/// corrupt metadata, so consumers treat the offsets as unusable and fall back
/// to searching the file contents.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum OffsetEncoding {
    /// UTF-8 (`utf-8`).
    Utf8,
    /// UTF-16, little endian (`utf-16-le`).
    Utf16Le,
    /// UTF-16, big endian (`utf-16-be`).
    Utf16Be,
    /// UTF-32, little endian (`utf-32-le`).
    Utf32Le,
    /// UTF-32, big endian (`utf-32-be`).
    Utf32Be,
    /// An encoding name not defined by the CEP.
    Unknown(String),
}

impl OffsetEncoding {
    /// The wire name of this encoding (e.g. `utf-8`).
    pub fn as_str(&self) -> &str {
        match self {
            OffsetEncoding::Utf8 => "utf-8",
            OffsetEncoding::Utf16Le => "utf-16-le",
            OffsetEncoding::Utf16Be => "utf-16-be",
            OffsetEncoding::Utf32Le => "utf-32-le",
            OffsetEncoding::Utf32Be => "utf-32-be",
            OffsetEncoding::Unknown(name) => name,
        }
    }
}

impl From<String> for OffsetEncoding {
    fn from(value: String) -> Self {
        match value.as_str() {
            "utf-8" => OffsetEncoding::Utf8,
            "utf-16-le" => OffsetEncoding::Utf16Le,
            "utf-16-be" => OffsetEncoding::Utf16Be,
            "utf-32-le" => OffsetEncoding::Utf32Le,
            "utf-32-be" => OffsetEncoding::Utf32Be,
            _ => OffsetEncoding::Unknown(value),
        }
    }
}

impl From<OffsetEncoding> for String {
    fn from(value: OffsetEncoding) -> Self {
        value.as_str().to_owned()
    }
}

/// One offset group of [`PrefixPlaceholder::experimental_offsets`]: where the placeholder
/// occurs in the file contents under one encoding.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct OffsetGroup {
    /// The encoding under which the recorded occurrences were found.
    pub encoding: OffsetEncoding,

    /// The byte offsets of the occurrences under [`Self::encoding`]. The
    /// shape is determined normatively by `file_mode` (see [`OffsetRanges`])
    /// and must not be empty: a group is present exactly when at least one
    /// occurrence is listed in it.
    pub ranges: OffsetRanges,

    /// Whether the group carried members other than `encoding` and `ranges`.
    ///
    /// The CEP defines exactly those two keys today; a future CEP may add
    /// more. Until a consumer implements such a CEP it cannot know whether an
    /// extra member changes the meaning of the group, so the group must be
    /// treated like corrupt metadata (fall back to searching). Unrecognized
    /// members are not preserved on re-serialization.
    #[serde(skip)]
    pub has_unknown_members: bool,
}

impl<'de> Deserialize<'de> for OffsetGroup {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            encoding: OffsetEncoding,
            ranges: OffsetRanges,
            #[serde(flatten)]
            extra: std::collections::BTreeMap<String, serde_json::Value>,
        }

        let raw = Raw::deserialize(deserializer)?;
        Ok(OffsetGroup {
            encoding: raw.encoding,
            ranges: raw.ranges,
            has_unknown_members: !raw.extra.is_empty(),
        })
    }
}

/// The byte offsets recorded in one [`OffsetGroup`].
///
/// The shape depends on the file mode:
/// - **Text**: a flat list of byte positions (`[10, 45, 100]`).
/// - **Binary**: grouped by c-string — each inner array lists the prefix
///   offsets followed by the position of the first byte of the NUL terminator
///   (the encoding's zero code unit: one zero byte for UTF-8, two for UTF-16,
///   four for UTF-32), or the file size when the final c-string is
///   unterminated at end-of-file (`[[5, 39], [22, 30, 39]]`).
///
/// Occurrences inside the shebang region (the first
/// [`PrefixPlaceholder::experimental_shebang_length`] bytes) are excluded; the installer
/// transforms that region under its own shebang rules.
///
/// The shape is determined normatively by `file_mode`, not inferred from the
/// JSON structure: an empty text list and an empty binary list are
/// indistinguishable (and invalid — per the CEP, `ranges` must not be empty).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(untagged)]
pub enum OffsetRanges {
    /// Text-mode ranges: flat list of byte positions where the placeholder
    /// occurs under the group's encoding.
    Text(Vec<usize>),
    /// Binary-mode ranges: grouped by c-string. Each inner array contains
    /// the prefix start positions followed by the NUL terminator position.
    Binary(Vec<Vec<usize>>),
}

impl OffsetRanges {
    /// Whether no positions are recorded at all. Invalid per the CEP: a group
    /// is present exactly when at least one occurrence is listed in it.
    pub fn is_empty(&self) -> bool {
        match self {
            OffsetRanges::Text(offsets) => offsets.is_empty(),
            OffsetRanges::Binary(groups) => groups.is_empty(),
        }
    }
}

/// Error describing why recorded offset metadata cannot be used.
///
/// Per the CEP, consumers that hit this SHOULD fall back to locating
/// occurrences by searching the file contents (and MAY report a warning).
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct InvalidOffsetsError(String);

/// Select the UTF-8 offset group from `offsets`, validating the CEP's
/// structural rules.
///
/// Per the CEP, installers apply exactly the groups whose encodings their own
/// search-based replacement covers. rattler's search-based replacement covers
/// UTF-8 only, so this helper returns:
///
/// - `Ok(Some(ranges))` — the UTF-8 group's ranges, to be spliced;
/// - `Ok(None)` — the metadata is valid but records no UTF-8 occurrences
///   (only wide-string groups, whose occurrences rattler's own search would
///   not have replaced either, or — for a text file with a `shebang_length` —
///   an empty list because every occurrence falls within the shebang region);
/// - `Err(_)` — the metadata is structurally invalid or contains values a
///   future CEP may have redefined; the caller falls back to searching.
///
/// Validated here: the list is non-empty (except for `file_mode: text`
/// entries with a `shebang_length`), every group's encoding is recognized, no
/// group carries unrecognized members, encodings are unique, and `ranges` are
/// non-empty with a shape matching `file_mode`. Value-level consistency of
/// the UTF-8 ranges with the actual file contents (ordering, bounds, the
/// placeholder bytes being present) is checked by the replacement functions
/// themselves.
pub fn select_utf8_offset_ranges(
    offsets: &[OffsetGroup],
    file_mode: FileMode,
    has_shebang_length: bool,
) -> Result<Option<&OffsetRanges>, InvalidOffsetsError> {
    if offsets.is_empty() {
        // An empty list is only meaningful for a text file whose every
        // occurrence lies inside the shebang region.
        return if file_mode == FileMode::Text && has_shebang_length {
            Ok(None)
        } else {
            Err(InvalidOffsetsError(String::from(
                "the offsets list is empty, which is only valid for a text file with a \
                 shebang_length",
            )))
        };
    }

    let mut utf8_ranges = None;
    let mut seen: Vec<&OffsetEncoding> = Vec::with_capacity(offsets.len());
    for group in offsets {
        if let OffsetEncoding::Unknown(name) = &group.encoding {
            return Err(InvalidOffsetsError(format!(
                "unrecognized encoding '{name}'"
            )));
        }
        if group.has_unknown_members {
            return Err(InvalidOffsetsError(format!(
                "the '{}' group has unrecognized members",
                group.encoding.as_str()
            )));
        }
        if seen.contains(&&group.encoding) {
            return Err(InvalidOffsetsError(format!(
                "duplicate '{}' groups",
                group.encoding.as_str()
            )));
        }
        seen.push(&group.encoding);
        if group.ranges.is_empty() {
            return Err(InvalidOffsetsError(format!(
                "the '{}' group's ranges are empty",
                group.encoding.as_str()
            )));
        }
        let shape_matches = matches!(
            (file_mode, &group.ranges),
            (FileMode::Text, OffsetRanges::Text(_)) | (FileMode::Binary, OffsetRanges::Binary(_))
        );
        if !shape_matches {
            return Err(InvalidOffsetsError(format!(
                "the shape of the '{}' group's ranges does not match the file mode",
                group.encoding.as_str()
            )));
        }
        if group.encoding == OffsetEncoding::Utf8 {
            utf8_ranges = Some(&group.ranges);
        }
    }
    Ok(utf8_ranges)
}

/// Deserializes `offsets` leniently: a value that does not parse as a list of
/// [`OffsetGroup`]s (for example the flat `[10, 45]` / `[[64, 96]]` lists
/// written by pre-CEP drafts of this field) yields `None` instead of failing
/// the whole `paths.json`. The field is advisory — the search-based path
/// handles the file correctly without it.
fn deserialize_offset_groups<'de, D>(deserializer: D) -> Result<Option<Vec<OffsetGroup>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
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

    use super::{
        FileMode, OffsetEncoding, OffsetGroup, OffsetRanges, PathBuf, PathType, PathsEntry,
        PathsJson, select_utf8_offset_ranges,
    };

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

        insta::assert_yaml_snapshot!(
            PathsJson::from_deprecated_package_directory(package_dir.path()).unwrap()
        );
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
    pub fn test_deserialize_paths_json_with_offsets() {
        let package_dir = tempfile::tempdir().unwrap();
        let info_dir = package_dir.path().join("info");
        std::fs::create_dir_all(&info_dir).unwrap();

        // Create a mock paths.json with offset fields
        let paths_json = r#"{
            "paths": [
                {
                    "_path": "bin/example",
                    "no_link": false,
                    "path_type": "hardlink",
                    "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    "size_in_bytes": 1024,
                    "file_mode": "binary",
                    "prefix_placeholder": "/opt/conda",
                    "offsets": [
                        {"encoding": "utf-16-le", "ranges": [[900, 1000]]},
                        {"encoding": "utf-8", "ranges": [[100, 500], [200, 300, 800]]}
                    ]
                },
                {
                    "_path": "lib/library.so",
                    "no_link": false,
                    "path_type": "hardlink",
                    "sha256": "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592",
                    "size_in_bytes": 2048
                },
                {
                    "_path": "share/doc/readme.txt",
                    "no_link": false,
                    "path_type": "hardlink",
                    "sha256": "a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3",
                    "size_in_bytes": 256,
                    "file_mode": "text",
                    "prefix_placeholder": "/home/builder/conda",
                    "offsets": [{"encoding": "utf-8", "ranges": [10, 45]}]
                },
                {
                    "_path": "bin/symlink-example",
                    "no_link": false,
                    "path_type": "softlink"
                }
            ],
            "paths_version": 1
            }"#;

        // Write the mock paths.json
        std::fs::write(info_dir.join("paths.json"), paths_json).unwrap();

        // Test loading it
        let paths_json =
            PathsJson::from_package_directory_with_deprecated_fallback(package_dir.path()).unwrap();

        assert_eq!(paths_json.paths_version, 1);
        assert_eq!(paths_json.paths.len(), 4);

        // First entry: binary with offset groups under two encodings. rattler
        // applies the utf-8 group only; the utf-16-le group is recorded for
        // installers whose search-based replacement covers wide strings.
        assert_eq!(
            paths_json.paths[0].relative_path,
            PathBuf::from("bin/example")
        );
        assert_eq!(paths_json.paths[0].size_in_bytes, Some(1024));
        let prefix = paths_json.paths[0].prefix_placeholder.as_ref().unwrap();
        assert_eq!(prefix.file_mode, FileMode::Binary);
        assert_eq!(
            prefix.experimental_offsets,
            Some(vec![
                OffsetGroup {
                    encoding: OffsetEncoding::Utf16Le,
                    ranges: OffsetRanges::Binary(vec![vec![900, 1000]]),
                    has_unknown_members: false,
                },
                OffsetGroup {
                    encoding: OffsetEncoding::Utf8,
                    ranges: OffsetRanges::Binary(vec![vec![100, 500], vec![200, 300, 800]]),
                    has_unknown_members: false,
                },
            ])
        );
        assert_eq!(
            select_utf8_offset_ranges(
                prefix.experimental_offsets.as_deref().unwrap(),
                prefix.file_mode,
                prefix.experimental_shebang_length.is_some()
            )
            .unwrap(),
            Some(&OffsetRanges::Binary(vec![
                vec![100, 500],
                vec![200, 300, 800]
            ]))
        );

        // Second entry: no prefix placeholder
        assert!(paths_json.paths[1].prefix_placeholder.is_none());

        // Third entry: text with offsets
        let text_prefix = paths_json.paths[2].prefix_placeholder.as_ref().unwrap();
        assert_eq!(text_prefix.file_mode, FileMode::Text);
        assert_eq!(
            text_prefix.experimental_offsets,
            Some(vec![OffsetGroup {
                encoding: OffsetEncoding::Utf8,
                ranges: OffsetRanges::Text(vec![10, 45]),
                has_unknown_members: false,
            }])
        );

        // Fourth entry: symlink, no offsets
        assert_eq!(paths_json.paths[3].path_type, PathType::SoftLink);
        assert!(paths_json.paths[3].prefix_placeholder.is_none());

        insta::assert_yaml_snapshot!(paths_json);
    }

    #[test]
    pub fn test_optional_fields_handling() {
        let package_dir = tempfile::tempdir().unwrap();
        let info_dir = package_dir.path().join("info");
        std::fs::create_dir_all(&info_dir).unwrap();

        // Test that the fields are truly optional
        let minimal = r#"{
            "paths": [
                {
                "_path": "file.txt",
                "path_type": "hardlink"
                }
            ],
            "paths_version": 1
            }"#;

        std::fs::write(info_dir.join("paths.json"), minimal).unwrap();

        let paths_json = PathsJson::from_package_directory(package_dir.path()).unwrap();

        assert_eq!(paths_json.paths_version, 1);
        assert_eq!(paths_json.paths[0].sha256, None);
        assert_eq!(paths_json.paths[0].size_in_bytes, None);
        assert!(paths_json.paths[0].prefix_placeholder.is_none());
    }

    #[test]
    pub fn test_serialization_roundtrip() {
        // Create a PathsJson with offset fields programmatically
        let original = PathsJson {
            paths: vec![
                PathsEntry {
                    relative_path: PathBuf::from("bin/tool"),
                    no_link: false,
                    path_type: PathType::HardLink,
                    prefix_placeholder: Some(PrefixPlaceholder {
                        file_mode: FileMode::Binary,
                        placeholder: "/opt/conda".to_string(),
                        experimental_offsets: Some(vec![OffsetGroup {
                            encoding: OffsetEncoding::Utf8,
                            ranges: OffsetRanges::Binary(vec![vec![50, 200], vec![150, 200]]),
                            has_unknown_members: false,
                        }]),
                        experimental_shebang_length: None,
                    }),
                    sha256: None,
                    size_in_bytes: Some(4096),
                },
                PathsEntry {
                    relative_path: PathBuf::from("lib/module.py"),
                    no_link: false,
                    path_type: PathType::HardLink,
                    prefix_placeholder: None,
                    sha256: None,
                    size_in_bytes: Some(512),
                },
            ],
            paths_version: 1,
        };

        // Serialize to JSON
        let json = serde_json::to_string_pretty(&original).unwrap();

        // Deserialize back
        let deserialized: PathsJson = serde_json::from_str(&json).unwrap();

        // Verify roundtrip
        assert_eq!(original, deserialized);
        assert_eq!(deserialized.paths_version, 1);
        assert_eq!(
            deserialized.paths[0]
                .prefix_placeholder
                .as_ref()
                .unwrap()
                .experimental_offsets,
            Some(vec![OffsetGroup {
                encoding: OffsetEncoding::Utf8,
                ranges: OffsetRanges::Binary(vec![vec![50, 200], vec![150, 200]]),
                has_unknown_members: false,
            }])
        );
    }

    /// The two path-entry examples from the CEP's Examples section must
    /// deserialize as written there.
    #[test]
    pub fn test_deserialize_cep_examples() {
        let text_entry = r#"{
            "_path": "bin/example-script",
            "path_type": "hardlink",
            "file_mode": "text",
            "prefix_placeholder": "/opt/placeholder",
            "offsets": [{"encoding": "utf-8", "ranges": [71]}],
            "shebang_length": 30,
            "sha256": "a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3",
            "size_in_bytes": 512
        }"#;
        let entry: PathsEntry = serde_json::from_str(text_entry).unwrap();
        let placeholder = entry.prefix_placeholder.as_ref().unwrap();
        assert_eq!(placeholder.experimental_shebang_length, Some(30));
        assert_eq!(
            select_utf8_offset_ranges(
                placeholder.experimental_offsets.as_deref().unwrap(),
                FileMode::Text,
                true
            )
            .unwrap(),
            Some(&OffsetRanges::Text(vec![71]))
        );

        let binary_entry = r#"{
            "_path": "lib/libexample.so",
            "path_type": "hardlink",
            "file_mode": "binary",
            "prefix_placeholder": "/opt/placeholder",
            "offsets": [
                {"encoding": "utf-16-le", "ranges": [[384, 448]]},
                {"encoding": "utf-8", "ranges": [[64, 96], [200, 240, 300]]}
            ],
            "sha256": "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592",
            "size_in_bytes": 4096
        }"#;
        let entry: PathsEntry = serde_json::from_str(binary_entry).unwrap();
        let placeholder = entry.prefix_placeholder.as_ref().unwrap();
        assert_eq!(
            select_utf8_offset_ranges(
                placeholder.experimental_offsets.as_deref().unwrap(),
                FileMode::Binary,
                false
            )
            .unwrap(),
            Some(&OffsetRanges::Binary(vec![
                vec![64, 96],
                vec![200, 240, 300]
            ]))
        );
    }

    /// The flat lists written by pre-CEP drafts of the `offsets` field do not
    /// parse as offset groups; they must be treated as absent rather than
    /// failing the whole `paths.json`.
    #[test]
    pub fn test_pre_cep_flat_offsets_treated_as_absent() {
        for old_format in [r#"[10, 45]"#, r#"[[100, 500], [200, 300, 800]]"#] {
            let entry = format!(
                r#"{{
                    "_path": "bin/example",
                    "path_type": "hardlink",
                    "file_mode": "text",
                    "prefix_placeholder": "/opt/conda",
                    "offsets": {old_format}
                }}"#
            );
            let entry: PathsEntry = serde_json::from_str(&entry).unwrap();
            assert_eq!(
                entry
                    .prefix_placeholder
                    .as_ref()
                    .unwrap()
                    .experimental_offsets,
                None,
                "old-format offsets {old_format} should deserialize as absent"
            );
        }
    }

    /// An encoding name outside the CEP's closed set parses (it must not fail
    /// the whole `paths.json`) but makes the metadata unusable, so selection
    /// reports an error and the consumer falls back to searching.
    #[test]
    pub fn test_unknown_encoding_parses_but_is_rejected_by_selection() {
        let groups: Vec<OffsetGroup> =
            serde_json::from_str(r#"[{"encoding": "utf-64-xe", "ranges": [10]}]"#).unwrap();
        assert_eq!(
            groups[0].encoding,
            OffsetEncoding::Unknown(String::from("utf-64-xe"))
        );
        assert!(select_utf8_offset_ranges(&groups, FileMode::Text, false).is_err());
    }

    /// A group member beyond `encoding` and `ranges` parses but marks the
    /// group, and selection rejects it: a future CEP may have changed the
    /// group's meaning, so it must be treated like corrupt metadata.
    #[test]
    pub fn test_unknown_group_member_is_rejected_by_selection() {
        let groups: Vec<OffsetGroup> =
            serde_json::from_str(r#"[{"encoding": "utf-8", "ranges": [10], "padding": "zero"}]"#)
                .unwrap();
        assert!(groups[0].has_unknown_members);
        assert!(select_utf8_offset_ranges(&groups, FileMode::Text, false).is_err());
    }

    #[test]
    pub fn test_select_utf8_offset_ranges_validation() {
        let utf8_text = OffsetGroup {
            encoding: OffsetEncoding::Utf8,
            ranges: OffsetRanges::Text(vec![10]),
            has_unknown_members: false,
        };
        let utf16_binary = OffsetGroup {
            encoding: OffsetEncoding::Utf16Le,
            ranges: OffsetRanges::Binary(vec![vec![384, 448]]),
            has_unknown_members: false,
        };

        // No utf-8 group is valid metadata with nothing to splice.
        assert_eq!(
            select_utf8_offset_ranges(std::slice::from_ref(&utf16_binary), FileMode::Binary, false)
                .unwrap(),
            None
        );

        // An empty list is only valid for a text file with a shebang_length.
        assert_eq!(
            select_utf8_offset_ranges(&[], FileMode::Text, true).unwrap(),
            None
        );
        assert!(select_utf8_offset_ranges(&[], FileMode::Text, false).is_err());
        assert!(select_utf8_offset_ranges(&[], FileMode::Binary, false).is_err());

        // Duplicate encodings are rejected.
        assert!(
            select_utf8_offset_ranges(
                &[utf8_text.clone(), utf8_text.clone()],
                FileMode::Text,
                false
            )
            .is_err()
        );

        // Empty ranges are rejected.
        assert!(
            select_utf8_offset_ranges(
                &[OffsetGroup {
                    encoding: OffsetEncoding::Utf8,
                    ranges: OffsetRanges::Text(vec![]),
                    has_unknown_members: false,
                }],
                FileMode::Text,
                false
            )
            .is_err()
        );

        // A ranges shape that does not match the file mode is rejected, even
        // on a group the installer would not apply.
        assert!(
            select_utf8_offset_ranges(std::slice::from_ref(&utf8_text), FileMode::Binary, false)
                .is_err()
        );
        assert!(
            select_utf8_offset_ranges(std::slice::from_ref(&utf16_binary), FileMode::Text, false)
                .is_err()
        );
    }

    #[test]
    pub fn test_fallback_from_v1_to_deprecated() {
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

        // Deprecated format shouldn't have offsets
        assert!(paths_json.paths.iter().all(|p| {
            p.prefix_placeholder
                .as_ref()
                .is_none_or(|pp| pp.experimental_offsets.is_none())
        }));
    }
}
