use super::PackageFile;
use crate::{
    package::has_prefix::HasPrefixEntry,
    package::{Files, HasPrefix, NoLink, NoSoftlink},
};
use rattler_digest::serde::SerializableHash;
use rattler_macros::sorted;
use serde::{Deserialize, Serialize, Serializer};
use serde_with::serde_as;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::str::FromStr;

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
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PrefixPlaceholder {
    /// The type of the file, either binary or text. Depending on the type of file either text
    /// replacement is performed or `CString` replacement.
    pub file_mode: FileMode,

    /// The placeholder prefix used in the file. This is the path of the prefix when the package
    /// was build.
    pub placeholder: String,

    /// The placeholder's occurrences in the file, as recorded by the producer in the `offsets`
    /// and `shebang_length` keys proposed by the [draft CEP].
    ///
    /// `None` when the package records no offsets, or records them in a form that does not parse
    /// as offset groups (such as the flat lists written by earlier drafts of the field); callers
    /// locate the occurrences by searching the file contents. `Some(Err(_))` when the recorded
    /// offsets violate the draft CEP; callers search the file contents as well and may report
    /// the error. Only valid offsets recorded for [`Self::file_mode`] are serialized.
    ///
    /// **Experimental**: the Rust field is prefixed until
    /// [conda/ceps#179](https://github.com/conda/ceps/pull/179) is finalized.
    ///
    /// [draft CEP]: https://github.com/conda/ceps/pull/179
    pub experimental_offsets: Option<Result<PrefixOffsets, InvalidOffsetsError>>,
}

impl Serialize for PrefixPlaceholder {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Offsets recorded for another file mode no longer describe the file.
        let offsets = match &self.experimental_offsets {
            Some(Ok(offsets)) if offsets.file_mode() == self.file_mode => Some(offsets),
            Some(Ok(_) | Err(_)) | None => None,
        };
        SerializedPrefixPlaceholder {
            file_mode: self.file_mode,
            placeholder: &self.placeholder,
            offsets: offsets.map(PrefixOffsets::groups),
            shebang_length: offsets.and_then(PrefixOffsets::shebang_length),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PrefixPlaceholder {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawPrefixPlaceholder::deserialize(deserializer)?;
        let experimental_offsets = raw.offsets.map(|groups| {
            let shebang_length = raw
                .shebang_length
                .map(|value| {
                    value
                        .as_u64()
                        .and_then(|length| usize::try_from(length).ok())
                        .ok_or(InvalidOffsetsError::MalformedShebangLength)
                })
                .transpose()?;
            let groups = groups
                .into_iter()
                .map(RawOffsetGroup::into_offset_group)
                .collect::<Result<Vec<_>, _>>()?;
            PrefixOffsets::new(raw.file_mode, groups, shebang_length)
        });
        Ok(PrefixPlaceholder {
            file_mode: raw.file_mode,
            placeholder: raw.placeholder,
            experimental_offsets,
        })
    }
}

/// The serialized form of a [`PrefixPlaceholder`].
#[derive(Serialize)]
struct SerializedPrefixPlaceholder<'a> {
    file_mode: FileMode,
    #[serde(rename = "prefix_placeholder")]
    placeholder: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    offsets: Option<&'a [OffsetGroup]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shebang_length: Option<usize>,
}

/// A [`PrefixPlaceholder`] as it appears in `paths.json`, before the offsets are validated.
#[derive(Deserialize)]
struct RawPrefixPlaceholder {
    file_mode: FileMode,
    #[serde(rename = "prefix_placeholder")]
    placeholder: String,
    #[serde(default, deserialize_with = "deserialize_offset_groups")]
    offsets: Option<Vec<RawOffsetGroup>>,
    /// Kept as a raw value so that a malformed length invalidates the offsets rather than the
    /// whole placeholder.
    #[serde(default)]
    shebang_length: Option<serde_json::Value>,
}

/// An [`OffsetGroup`] as it appears in `paths.json`, before it is validated.
#[derive(Deserialize)]
struct RawOffsetGroup {
    encoding: String,
    ranges: OffsetRanges,
    #[serde(flatten)]
    other_members: BTreeMap<String, serde_json::Value>,
}

impl RawOffsetGroup {
    /// Validates the group. A member other than `encoding` and `ranges` may change the meaning
    /// of the group in a future revision of the draft CEP, so it invalidates the group.
    fn into_offset_group(self) -> Result<OffsetGroup, InvalidOffsetsError> {
        let encoding = self.encoding.parse::<OffsetEncoding>()?;
        if !self.other_members.is_empty() {
            return Err(InvalidOffsetsError::UnrecognizedMembers {
                encoding,
                members: self.other_members.into_keys().collect(),
            });
        }
        OffsetGroup::new(encoding, self.ranges)
    }
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

/// The encoding of one [`OffsetGroup`], from the closed set the [draft CEP] defines: the
/// encodings replaced by existing installers.
///
/// [draft CEP]: https://github.com/conda/ceps/pull/179
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum OffsetEncoding {
    /// UTF-8 (`utf-8`).
    #[serde(rename = "utf-8")]
    Utf8,
    /// UTF-16, little endian (`utf-16-le`).
    #[serde(rename = "utf-16-le")]
    Utf16Le,
    /// UTF-16, big endian (`utf-16-be`).
    #[serde(rename = "utf-16-be")]
    Utf16Be,
    /// UTF-32, little endian (`utf-32-le`).
    #[serde(rename = "utf-32-le")]
    Utf32Le,
    /// UTF-32, big endian (`utf-32-be`).
    #[serde(rename = "utf-32-be")]
    Utf32Be,
}

impl OffsetEncoding {
    /// Every encoding the draft CEP defines.
    pub const DEFINED: [OffsetEncoding; 5] = [
        OffsetEncoding::Utf8,
        OffsetEncoding::Utf16Le,
        OffsetEncoding::Utf16Be,
        OffsetEncoding::Utf32Le,
        OffsetEncoding::Utf32Be,
    ];

    /// The name of this encoding in `paths.json` (e.g. `utf-8`).
    pub fn as_str(self) -> &'static str {
        match self {
            OffsetEncoding::Utf8 => "utf-8",
            OffsetEncoding::Utf16Le => "utf-16-le",
            OffsetEncoding::Utf16Be => "utf-16-be",
            OffsetEncoding::Utf32Le => "utf-32-le",
            OffsetEncoding::Utf32Be => "utf-32-be",
        }
    }

    /// The size in bytes of one code unit of this encoding, which is also the size of the NUL
    /// terminator of a c-string stored in it.
    pub fn code_unit_size(self) -> usize {
        match self {
            OffsetEncoding::Utf8 => 1,
            OffsetEncoding::Utf16Le | OffsetEncoding::Utf16Be => 2,
            OffsetEncoding::Utf32Le | OffsetEncoding::Utf32Be => 4,
        }
    }

    /// Encodes `text` with this encoding, without a byte order mark.
    pub fn encode(self, text: &str) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(text.len() * self.code_unit_size());
        match self {
            OffsetEncoding::Utf8 => bytes.extend_from_slice(text.as_bytes()),
            OffsetEncoding::Utf16Le => bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes)),
            OffsetEncoding::Utf16Be => bytes.extend(text.encode_utf16().flat_map(u16::to_be_bytes)),
            OffsetEncoding::Utf32Le => {
                bytes.extend(text.chars().flat_map(|c| u32::from(c).to_le_bytes()));
            }
            OffsetEncoding::Utf32Be => {
                bytes.extend(text.chars().flat_map(|c| u32::from(c).to_be_bytes()));
            }
        }
        bytes
    }
}

impl fmt::Display for OffsetEncoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for OffsetEncoding {
    type Err = InvalidOffsetsError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        OffsetEncoding::DEFINED
            .into_iter()
            .find(|encoding| encoding.as_str() == name)
            .ok_or_else(|| InvalidOffsetsError::UnrecognizedEncoding(name.to_owned()))
    }
}

/// Where the placeholder occurs in a file under one encoding, as defined by the [draft CEP].
///
/// A group always lists at least one occurrence, and every c-string of a binary group lists at
/// least one occurrence followed by its terminator.
///
/// [draft CEP]: https://github.com/conda/ceps/pull/179
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct OffsetGroup {
    encoding: OffsetEncoding,
    ranges: OffsetRanges,
}

impl OffsetGroup {
    /// Creates a group of the occurrences recorded under `encoding`.
    ///
    /// Fails when `ranges` is empty or when a c-string of binary ranges does not list both an
    /// occurrence and its terminator.
    pub fn new(
        encoding: OffsetEncoding,
        ranges: OffsetRanges,
    ) -> Result<Self, InvalidOffsetsError> {
        if ranges.is_empty() {
            return Err(InvalidOffsetsError::EmptyRanges(encoding));
        }
        if let OffsetRanges::Binary(cstrings) = &ranges
            && cstrings.iter().any(|cstring| cstring.len() < 2)
        {
            return Err(InvalidOffsetsError::ShortCStringRanges(encoding));
        }
        Ok(OffsetGroup { encoding, ranges })
    }

    /// The encoding under which the occurrences were recorded.
    pub fn encoding(&self) -> OffsetEncoding {
        self.encoding
    }

    /// The byte offsets of the occurrences, never empty.
    pub fn ranges(&self) -> &OffsetRanges {
        &self.ranges
    }
}

/// The byte offsets recorded in one [`OffsetGroup`].
///
/// The shape depends on the file mode:
/// - **Text**: a flat list of byte positions (`[10, 45, 100]`).
/// - **Binary**: grouped by c-string. Each inner array lists the prefix
///   offsets followed by the position of the first byte of the NUL terminator
///   (the encoding's zero code unit), or the file size when the final
///   c-string is unterminated at end-of-file (`[[5, 39], [22, 30, 39]]`).
///
/// Occurrences inside the shebang region (the first
/// [`PrefixOffsets::shebang_length`] bytes) are excluded; the installer
/// transforms that region under its own shebang rules.
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
    /// Whether no positions are recorded at all.
    pub fn is_empty(&self) -> bool {
        match self {
            OffsetRanges::Text(offsets) => offsets.is_empty(),
            OffsetRanges::Binary(groups) => groups.is_empty(),
        }
    }
}

/// The offsets of the placeholder in one file, recorded per encoding as defined by the
/// [draft CEP].
///
/// The groups have distinct encodings and the ranges shape of [`Self::file_mode`]. A list
/// without groups only occurs for a text file whose occurrences all lie inside its shebang
/// region, and [`Self::shebang_length`] is only recorded for text files. Whether the offsets
/// match the file contents (ordering, bounds, the placeholder bytes being present) can only be
/// checked against those contents, which the prefix replacement in `rattler` does. That check
/// only reads the bytes the offsets point at, so the offsets are trusted to list every occurrence
/// and the first terminator after each binary occurrence: an occurrence they leave out keeps the
/// placeholder, and a terminator recorded past the real one moves the bytes in between.
///
/// [draft CEP]: https://github.com/conda/ceps/pull/179
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PrefixOffsets {
    file_mode: FileMode,
    groups: Vec<OffsetGroup>,
    shebang_length: Option<usize>,
}

impl PrefixOffsets {
    /// Creates the offsets recorded for a file with the given `file_mode`.
    pub fn new(
        file_mode: FileMode,
        groups: Vec<OffsetGroup>,
        shebang_length: Option<usize>,
    ) -> Result<Self, InvalidOffsetsError> {
        let has_shebang_length = shebang_length.is_some();
        if has_shebang_length && file_mode == FileMode::Binary {
            return Err(InvalidOffsetsError::ShebangLengthOnBinary);
        }
        if groups.is_empty() && !has_shebang_length {
            return Err(InvalidOffsetsError::EmptyList);
        }

        for (index, group) in groups.iter().enumerate() {
            if groups[..index]
                .iter()
                .any(|earlier| earlier.encoding == group.encoding)
            {
                return Err(InvalidOffsetsError::DuplicateEncoding(group.encoding));
            }
            let shape_matches = match (file_mode, &group.ranges) {
                (FileMode::Text, OffsetRanges::Text(_))
                | (FileMode::Binary, OffsetRanges::Binary(_)) => true,
                (FileMode::Text, OffsetRanges::Binary(_))
                | (FileMode::Binary, OffsetRanges::Text(_)) => false,
            };
            if !shape_matches {
                return Err(InvalidOffsetsError::RangesShapeMismatch(group.encoding));
            }
        }

        Ok(PrefixOffsets {
            file_mode,
            groups,
            shebang_length,
        })
    }

    /// The file mode the offsets were recorded for.
    pub fn file_mode(&self) -> FileMode {
        self.file_mode
    }

    /// The recorded groups, one per encoding under which the placeholder occurs outside the
    /// shebang region.
    pub fn groups(&self) -> &[OffsetGroup] {
        &self.groups
    }

    /// The length in bytes of the file's shebang region: the first line including its
    /// terminating newline, or the whole file size when the file contains no newline.
    ///
    /// Recorded exactly when the file is a text file that starts with the bytes `#!`, whether
    /// or not the first line contains the placeholder.
    pub fn shebang_length(&self) -> Option<usize> {
        self.shebang_length
    }
}

/// Why recorded offsets violate the [draft CEP].
///
/// Consumers that hit this locate the occurrences by searching the file contents instead.
///
/// [draft CEP]: https://github.com/conda/ceps/pull/179
#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
pub enum InvalidOffsetsError {
    /// The offsets list is empty, which is only valid for a text file whose
    /// occurrences all lie inside the shebang region.
    #[error("the offsets list is empty, which is only valid for a text file with a shebang_length")]
    EmptyList,

    /// A group's encoding is not in the closed set defined by the draft CEP.
    #[error("unrecognized encoding '{0}'")]
    UnrecognizedEncoding(String),

    /// A group has members beyond `encoding` and `ranges`.
    #[error("the '{encoding}' group has unrecognized members: {}", members.join(", "))]
    UnrecognizedMembers {
        /// The group's encoding.
        encoding: OffsetEncoding,
        /// The names of the unrecognized members.
        members: Vec<String>,
    },

    /// Two groups share the same encoding.
    #[error("duplicate '{0}' groups")]
    DuplicateEncoding(OffsetEncoding),

    /// A group's ranges are empty.
    #[error("the '{0}' group's ranges are empty")]
    EmptyRanges(OffsetEncoding),

    /// A group's ranges have the text shape for a binary file or the binary
    /// shape for a text file.
    #[error("the shape of the '{0}' group's ranges does not match the file mode")]
    RangesShapeMismatch(OffsetEncoding),

    /// A binary c-string lists fewer than two values, so it has no occurrence
    /// or no terminator.
    #[error("a c-string of the '{0}' group lists fewer than two values")]
    ShortCStringRanges(OffsetEncoding),

    /// `shebang_length` is recorded for an entry that is not a text file.
    #[error("shebang_length is only valid for a text file")]
    ShebangLengthOnBinary,

    /// `shebang_length` is recorded but is not a byte length.
    #[error("shebang_length is not a byte length")]
    MalformedShebangLength,
}

/// Deserializes `offsets` leniently: a value that does not parse as a list of
/// offset groups (for example the flat `[10, 45]` / `[[64, 96]]` lists
/// written by earlier drafts of this field) yields `None` instead of failing
/// the whole `paths.json`. The field is advisory; the search-based path
/// handles the file correctly without it.
fn deserialize_offset_groups<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<RawOffsetGroup>>, D::Error>
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
    use std::path::Path;

    use crate::package::{PackageFile, PrefixPlaceholder};

    use super::{
        FileMode, InvalidOffsetsError, OffsetEncoding, OffsetGroup, OffsetRanges, PathBuf,
        PathType, PathsEntry, PathsJson, PrefixOffsets,
    };

    /// Builds a valid offset group.
    fn group(encoding: OffsetEncoding, ranges: OffsetRanges) -> OffsetGroup {
        OffsetGroup::new(encoding, ranges).unwrap()
    }

    /// Deserializes the offsets recorded for a placeholder with the given `file_mode`.
    fn deserialize_offsets(
        file_mode: &str,
        offsets: &str,
    ) -> Option<Result<PrefixOffsets, InvalidOffsetsError>> {
        let entry = format!(
            r#"{{
                "_path": "bin/example",
                "path_type": "hardlink",
                "file_mode": "{file_mode}",
                "prefix_placeholder": "/opt/conda",
                "offsets": {offsets}
            }}"#
        );
        let entry: PathsEntry = serde_json::from_str(&entry).unwrap();
        entry.prefix_placeholder.unwrap().experimental_offsets
    }

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
                relative_path: Path::new("rel").join(format!("path_{i}")),
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

        // First entry: binary with offset groups under two encodings.
        assert_eq!(
            paths_json.paths[0].relative_path,
            PathBuf::from("bin/example")
        );
        assert_eq!(paths_json.paths[0].size_in_bytes, Some(1024));
        let prefix = paths_json.paths[0].prefix_placeholder.as_ref().unwrap();
        assert_eq!(prefix.file_mode, FileMode::Binary);
        assert_eq!(
            prefix.experimental_offsets,
            Some(Ok(PrefixOffsets::new(
                FileMode::Binary,
                vec![
                    group(
                        OffsetEncoding::Utf16Le,
                        OffsetRanges::Binary(vec![vec![900, 1000]])
                    ),
                    group(
                        OffsetEncoding::Utf8,
                        OffsetRanges::Binary(vec![vec![100, 500], vec![200, 300, 800]])
                    ),
                ],
                None
            )
            .unwrap()))
        );

        // Second entry: no prefix placeholder
        assert!(paths_json.paths[1].prefix_placeholder.is_none());

        // Third entry: text with offsets
        let text_prefix = paths_json.paths[2].prefix_placeholder.as_ref().unwrap();
        assert_eq!(text_prefix.file_mode, FileMode::Text);
        assert_eq!(
            text_prefix.experimental_offsets,
            Some(Ok(PrefixOffsets::new(
                FileMode::Text,
                vec![group(
                    OffsetEncoding::Utf8,
                    OffsetRanges::Text(vec![10, 45])
                )],
                None
            )
            .unwrap()))
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
        let offsets = PrefixOffsets::new(
            FileMode::Binary,
            vec![group(
                OffsetEncoding::Utf8,
                OffsetRanges::Binary(vec![vec![50, 200], vec![150, 200]]),
            )],
            None,
        )
        .unwrap();
        let original = PathsJson {
            paths: vec![
                PathsEntry {
                    relative_path: PathBuf::from("bin/tool"),
                    no_link: false,
                    path_type: PathType::HardLink,
                    prefix_placeholder: Some(PrefixPlaceholder {
                        file_mode: FileMode::Binary,
                        placeholder: "/opt/conda".to_string(),
                        experimental_offsets: Some(Ok(offsets.clone())),
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
            Some(Ok(offsets))
        );
    }

    /// The two path-entry examples from the draft CEP's Examples section must
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
        assert_eq!(
            placeholder.experimental_offsets,
            Some(Ok(PrefixOffsets::new(
                FileMode::Text,
                vec![group(OffsetEncoding::Utf8, OffsetRanges::Text(vec![71]))],
                Some(30)
            )
            .unwrap()))
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
            placeholder.experimental_offsets,
            Some(Ok(PrefixOffsets::new(
                FileMode::Binary,
                vec![
                    group(
                        OffsetEncoding::Utf16Le,
                        OffsetRanges::Binary(vec![vec![384, 448]])
                    ),
                    group(
                        OffsetEncoding::Utf8,
                        OffsetRanges::Binary(vec![vec![64, 96], vec![200, 240, 300]])
                    ),
                ],
                None
            )
            .unwrap()))
        );
    }

    /// The flat lists written by earlier drafts of the `offsets` field do not
    /// parse as offset groups; they must be treated as absent rather than
    /// failing the whole `paths.json`.
    #[test]
    pub fn test_pre_cep_flat_offsets_treated_as_absent() {
        for old_format in [r#"[10, 45]"#, r#"[[100, 500], [200, 300, 800]]"#] {
            assert_eq!(
                deserialize_offsets("text", old_format),
                None,
                "old-format offsets {old_format} should deserialize as absent"
            );
        }
    }

    /// An encoding name outside the draft CEP's closed set does not fail the
    /// whole `paths.json`, but makes the recorded offsets invalid so that the
    /// consumer falls back to searching.
    #[test]
    pub fn test_unknown_encoding_is_invalid() {
        assert_eq!(
            deserialize_offsets("text", r#"[{"encoding": "utf-64-xe", "ranges": [10]}]"#),
            Some(Err(InvalidOffsetsError::UnrecognizedEncoding(
                String::from("utf-64-xe")
            )))
        );
    }

    /// A group member beyond `encoding` and `ranges` makes the recorded
    /// offsets invalid: a future CEP may have changed the group's meaning, so
    /// it must be treated like corrupt metadata.
    #[test]
    pub fn test_unknown_group_member_is_invalid() {
        assert_eq!(
            deserialize_offsets(
                "text",
                r#"[{"encoding": "utf-8", "ranges": [10], "padding": "zero"}]"#
            ),
            Some(Err(InvalidOffsetsError::UnrecognizedMembers {
                encoding: OffsetEncoding::Utf8,
                members: vec![String::from("padding")],
            }))
        );
    }

    /// A malformed `shebang_length` invalidates the recorded offsets, but the
    /// placeholder itself is kept so that the file is still relocated.
    #[test]
    pub fn test_malformed_shebang_length_keeps_placeholder() {
        for shebang_length in ["-1", r#""30""#, "30.0", "18446744073709551616"] {
            let entry: PathsEntry = serde_json::from_str(&format!(
                r#"{{
                    "_path": "bin/example",
                    "path_type": "hardlink",
                    "file_mode": "text",
                    "prefix_placeholder": "/opt/conda",
                    "offsets": [{{"encoding": "utf-8", "ranges": [40]}}],
                    "shebang_length": {shebang_length}
                }}"#
            ))
            .unwrap();
            assert_eq!(
                entry
                    .prefix_placeholder
                    .map(|placeholder| placeholder.experimental_offsets),
                Some(Some(Err(InvalidOffsetsError::MalformedShebangLength))),
                "shebang_length {shebang_length}"
            );
        }
    }

    /// Offsets recorded for another file mode no longer describe the file, so
    /// they are not serialized.
    #[test]
    pub fn test_offsets_for_other_file_mode_are_not_serialized() {
        let placeholder = PrefixPlaceholder {
            file_mode: FileMode::Binary,
            placeholder: String::from("/opt/conda"),
            experimental_offsets: Some(Ok(PrefixOffsets::new(
                FileMode::Text,
                vec![group(OffsetEncoding::Utf8, OffsetRanges::Text(vec![10]))],
                None,
            )
            .unwrap())),
        };
        insta::assert_snapshot!(
            serde_json::to_string(&placeholder).unwrap(),
            @r#"{"file_mode":"binary","prefix_placeholder":"/opt/conda"}"#
        );
    }

    #[test]
    pub fn test_prefix_offsets_validation() {
        let utf8_text = group(OffsetEncoding::Utf8, OffsetRanges::Text(vec![10]));
        let utf16_binary = group(
            OffsetEncoding::Utf16Le,
            OffsetRanges::Binary(vec![vec![384, 448]]),
        );

        assert!(PrefixOffsets::new(FileMode::Binary, vec![utf16_binary.clone()], None).is_ok());

        // An empty list is only valid for a text file with a shebang_length.
        assert!(PrefixOffsets::new(FileMode::Text, vec![], Some(10)).is_ok());
        assert_eq!(
            PrefixOffsets::new(FileMode::Text, vec![], None),
            Err(InvalidOffsetsError::EmptyList)
        );
        assert_eq!(
            PrefixOffsets::new(FileMode::Binary, vec![], None),
            Err(InvalidOffsetsError::EmptyList)
        );

        // `shebang_length` is only valid for a text file.
        assert_eq!(
            PrefixOffsets::new(FileMode::Binary, vec![utf16_binary.clone()], Some(10)),
            Err(InvalidOffsetsError::ShebangLengthOnBinary)
        );

        // Every c-string must list at least one occurrence and its terminator.
        for cstring in [vec![], vec![384]] {
            assert_eq!(
                OffsetGroup::new(
                    OffsetEncoding::Utf8,
                    OffsetRanges::Binary(vec![cstring.clone()])
                ),
                Err(InvalidOffsetsError::ShortCStringRanges(
                    OffsetEncoding::Utf8
                )),
                "c-string {cstring:?}"
            );
        }

        assert_eq!(
            PrefixOffsets::new(
                FileMode::Text,
                vec![utf8_text.clone(), utf8_text.clone()],
                None
            ),
            Err(InvalidOffsetsError::DuplicateEncoding(OffsetEncoding::Utf8))
        );

        assert_eq!(
            OffsetGroup::new(OffsetEncoding::Utf8, OffsetRanges::Text(vec![])),
            Err(InvalidOffsetsError::EmptyRanges(OffsetEncoding::Utf8))
        );

        // A ranges shape that does not match the file mode is rejected.
        assert_eq!(
            PrefixOffsets::new(FileMode::Binary, vec![utf8_text], None),
            Err(InvalidOffsetsError::RangesShapeMismatch(
                OffsetEncoding::Utf8
            ))
        );
        assert_eq!(
            PrefixOffsets::new(FileMode::Text, vec![utf16_binary], None),
            Err(InvalidOffsetsError::RangesShapeMismatch(
                OffsetEncoding::Utf16Le
            ))
        );
    }

    /// The encodings of the draft CEP's closed set, as consumers encode the placeholder to search
    /// for and replace it. No byte order marks, and the code unit size doubles as the size of a
    /// c-string's NUL terminator.
    #[test]
    pub fn test_offset_encoding_encode() {
        assert_eq!(OffsetEncoding::Utf8.encode("/a"), b"/a");
        assert_eq!(OffsetEncoding::Utf16Le.encode("/a"), b"/\0a\0");
        assert_eq!(OffsetEncoding::Utf16Be.encode("/a"), b"\0/\0a");
        assert_eq!(OffsetEncoding::Utf32Le.encode("/a"), b"/\0\0\0a\0\0\0");
        assert_eq!(OffsetEncoding::Utf32Be.encode("/a"), b"\0\0\0/\0\0\0a");

        for encoding in OffsetEncoding::DEFINED {
            assert_eq!(encoding.encode("ab").len(), 2 * encoding.code_unit_size());
            assert_eq!(encoding.as_str().parse::<OffsetEncoding>(), Ok(encoding));
        }
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

    #[test]
    #[cfg(unix)]
    pub fn test_backslash_in_file_name() {
        // On Unix a backslash is an ordinary character in a file name, not a
        // path separator.
        let paths_json = PathsJson {
            paths: vec![PathsEntry {
                relative_path: Path::new("share").join("a\\b.txt"),
                path_type: super::PathType::HardLink,
                prefix_placeholder: None,
                no_link: false,
                sha256: None,
                size_in_bytes: Some(0),
            }],
            paths_version: 1,
        };

        let json = serde_json::to_string_pretty(&paths_json).unwrap();
        insta::assert_snapshot!(json, @r#"
        {
          "paths": [
            {
              "_path": "share/a\\b.txt",
              "path_type": "hardlink",
              "size_in_bytes": 0
            }
          ],
          "paths_version": 1
        }
        "#);
        assert_eq!(
            serde_json::from_str::<PathsJson>(&json).unwrap(),
            paths_json
        );
    }
}
