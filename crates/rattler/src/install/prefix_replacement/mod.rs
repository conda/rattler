//! Replaces the prefix placeholder recorded in `paths.json` with the target prefix.
//!
//! Two paths do the same job. The search-based one scans the file contents for the placeholder;
//! the offsets-based one splices the positions a producer recorded in `paths.json`, as proposed by
//! the [draft CEP]. Both build the same patch lists ([`TextPatch`], [`CStringPatch`]) and hand them
//! to the same splicers, which is what keeps them byte-for-byte identical.
//!
//! Text files live in [`text`], binary files in [`binary`], and this module holds what they share:
//! the per-encoding prefixes, the patch types and the two entry points that dispatch on
//! `file_mode`.
//!
//! [draft CEP]: https://github.com/conda/ceps/pull/179

use std::borrow::Cow;
use std::io::Write;

use rattler_conda_types::Subdir;
use rattler_conda_types::package::{FileMode, InvalidOffsetsError, OffsetEncoding, PrefixOffsets};

mod binary;
#[cfg(test)]
mod tests;
mod text;

pub use binary::{
    copy_and_replace_cstring_placeholder, copy_and_replace_cstring_placeholder_offsets,
    find_cstring_occurrences,
};
pub use text::{
    copy_and_replace_textual_placeholder, copy_and_replace_textual_placeholder_offsets,
    find_text_occurrences, replace_shebang_region,
};

/// Given the contents of a file copy it to the `destination` and in the process replace the
/// `prefix_placeholder` text with the `target_prefix` text.
///
/// This switches to more specialized functions that handle the replacement of either
/// textual and binary placeholders, the [`FileMode`] enum switches between the two functions.
/// See both [`copy_and_replace_cstring_placeholder`] and [`copy_and_replace_textual_placeholder`]
pub fn copy_and_replace_placeholders(
    source_bytes: &[u8],
    mut destination: impl Write,
    prefix_placeholder: &str,
    target_prefix: &str,
    target_platform: &Subdir,
    file_mode: FileMode,
) -> Result<(), std::io::Error> {
    match file_mode {
        FileMode::Text => {
            copy_and_replace_textual_placeholder(
                source_bytes,
                destination,
                prefix_placeholder,
                target_prefix,
                target_platform,
            )?;
        }
        FileMode::Binary => {
            // conda does not replace the prefix in the binary files on windows
            // DLLs are loaded quite differently anyways (there is no rpath, for example).
            if target_platform.is_windows() {
                destination.write_all(source_bytes)?;
            } else {
                copy_and_replace_cstring_placeholder(
                    source_bytes,
                    destination,
                    prefix_placeholder,
                    target_prefix,
                )?;
            }
        }
    }
    Ok(())
}

/// Error returned by the offset-based prefix replacement functions
/// ([`copy_and_replace_placeholders_with_offsets`] and the specialized
/// text/binary variants it dispatches to).
///
/// The offsets recorded in `paths.json` come from the package producer and
/// are not trusted. When they are inconsistent with the file contents the
/// install must not fail: the caller falls back to the search-based
/// replacement path. IO errors while writing the patched file are surfaced
/// separately.
///
/// The offset functions write nothing to the destination before returning
/// [`OffsetReplaceError::InconsistentMetadata`], so the caller can reuse the
/// same (still empty) destination for the fallback.
#[derive(Debug, thiserror::Error)]
pub enum OffsetReplaceError {
    /// The recorded offsets are inconsistent with the file contents. Callers
    /// should fall back to search-based replacement rather than failing the
    /// install.
    #[error("inconsistent prefix replacement metadata: {0}")]
    InconsistentMetadata(#[from] InconsistentOffsetsError),

    /// An IO error occurred while writing the patched file.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// How recorded offsets disagree with the file they were recorded for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InconsistentOffsetsError {
    /// The offsets violate the draft CEP regardless of the file contents.
    #[error(transparent)]
    Invalid(#[from] InvalidOffsetsError),

    /// The offsets were recorded for a different file mode than the one the file is installed
    /// with.
    #[error("the offsets were recorded for file mode {recorded:?}, not {installed:?}")]
    FileModeMismatch {
        /// The file mode the offsets were recorded for.
        recorded: FileMode,
        /// The file mode the file is installed with.
        installed: FileMode,
    },

    /// The placeholder is empty, so no recorded offset can point at it.
    #[error("the placeholder is empty")]
    EmptyPlaceholder,

    /// A `shebang_length` is recorded but the file does not start with `#!`.
    #[error("shebang_length is recorded but the file does not start with #!")]
    UnexpectedShebangLength,

    /// The file starts with `#!` but no `shebang_length` is recorded.
    #[error("the file starts with #! but no shebang_length is recorded")]
    MissingShebangLength,

    /// The recorded `shebang_length` is not the length of the file's first line.
    #[error("shebang_length {shebang_length} is not the length of the first line")]
    ShebangLengthMismatch {
        /// The recorded `shebang_length`.
        shebang_length: usize,
    },

    /// An offset lies inside the shebang region, which the draft CEP excludes from `offsets`.
    #[error("offset {offset} lies inside the shebang region of {shebang_length} bytes")]
    OffsetInShebangRegion {
        /// The recorded offset.
        offset: usize,
        /// The length of the shebang region.
        shebang_length: usize,
    },

    /// An offset starts before the end of the previous occurrence or c-string, across all
    /// encodings.
    #[error("offset {offset} overlaps or precedes the previous occurrence")]
    UnsortedOffset {
        /// The recorded offset.
        offset: usize,
    },

    /// The placeholder at an offset extends past the end of the file.
    #[error(
        "the placeholder at offset {offset} extends past the end of the file ({file_size} bytes)"
    )]
    OffsetOutOfRange {
        /// The recorded offset.
        offset: usize,
        /// The size of the file.
        file_size: usize,
    },

    /// The placeholder at an offset extends past the terminator of its c-string.
    #[error("the placeholder at offset {offset} extends past its terminator at {terminator}")]
    OffsetPastTerminator {
        /// The recorded offset.
        offset: usize,
        /// The recorded terminator position.
        terminator: usize,
    },

    /// An offset is not a whole number of code units before the terminator of its c-string.
    #[error(
        "offset {offset} is not a whole number of code units before its terminator at {terminator}"
    )]
    MisalignedOffset {
        /// The recorded offset.
        offset: usize,
        /// The recorded terminator position.
        terminator: usize,
    },

    /// The bytes at an offset are not the encoded placeholder.
    #[error("the placeholder is not present at offset {offset}")]
    PlaceholderNotFound {
        /// The recorded offset.
        offset: usize,
    },

    /// A terminator position lies past the end of the file.
    #[error("terminator {terminator} lies past the end of the file ({file_size} bytes)")]
    TerminatorOutOfRange {
        /// The recorded terminator position.
        terminator: usize,
        /// The size of the file.
        file_size: usize,
    },

    /// The bytes at a terminator position are not a zero code unit.
    #[error("the bytes at terminator {terminator} are not a zero code unit")]
    TerminatorNotZero {
        /// The recorded terminator position.
        terminator: usize,
    },
}

/// The placeholder and target prefix encoded with one of the encodings defined by the draft CEP.
///
/// Prefix replacement covers every one of these encodings, so paths that a binary stores as wide
/// strings are patched too.
struct EncodedPrefix {
    encoding: OffsetEncoding,
    placeholder: Vec<u8>,
    target: Vec<u8>,
}

impl EncodedPrefix {
    /// Encodes both prefixes with every encoding defined by the draft CEP, skipping encodings under
    /// which the placeholder is empty because there is nothing to search for.
    fn all(placeholder: &str, target: &str) -> Vec<EncodedPrefix> {
        OffsetEncoding::DEFINED
            .into_iter()
            .filter_map(|encoding| {
                let placeholder = encoding.encode(placeholder);
                (!placeholder.is_empty()).then(|| EncodedPrefix {
                    encoding,
                    placeholder,
                    target: encoding.encode(target),
                })
            })
            .collect()
    }

    /// The number of bytes one replacement frees up, or `None` when the target prefix is longer
    /// than the placeholder and therefore cannot be spliced into a fixed-size c-string.
    fn shrinks_by(&self) -> Option<usize> {
        self.placeholder.len().checked_sub(self.target.len())
    }

    /// The size of one code unit, which is also the size of a c-string's NUL terminator.
    fn code_unit_size(&self) -> usize {
        self.encoding.code_unit_size()
    }

    /// The error for a target prefix that does not fit in the space of the placeholder.
    fn growing_prefix_error(&self) -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "target prefix cannot be longer than the placeholder prefix (encoding '{}')",
                self.encoding
            ),
        )
    }
}

/// Rejects a target prefix that is longer than the placeholder under one of the encodings that is
/// actually replaced. Binary replacement preserves the file length, so the replacement must fit in
/// the space the placeholder occupies.
///
/// The lengths differ per encoding once the placeholder leaves ASCII (UTF-8 counts bytes, the wide
/// encodings count code units), so an encoding the file does not use must not fail the install.
fn reject_growing_prefix<'a>(
    prefixes: impl IntoIterator<Item = &'a EncodedPrefix>,
) -> Result<(), std::io::Error> {
    match prefixes
        .into_iter()
        .find(|prefix| prefix.shrinks_by().is_none())
    {
        Some(prefix) => Err(prefix.growing_prefix_error()),
        None => Ok(()),
    }
}

/// Looks up the encoded prefixes for the encoding of a recorded offset group.
fn encoded_prefix_for(
    prefixes: &[EncodedPrefix],
    encoding: OffsetEncoding,
) -> Result<&EncodedPrefix, InconsistentOffsetsError> {
    prefixes
        .iter()
        .find(|prefix| prefix.encoding == encoding)
        .ok_or(InconsistentOffsetsError::EmptyPlaceholder)
}

/// A placeholder occurrence in a text file that search-based replacement replaces, see
/// [`find_text_occurrences`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextOccurrence {
    /// Absolute byte position of the occurrence.
    pub offset: usize,
    /// The encoding the placeholder occurs in at `offset`.
    pub encoding: OffsetEncoding,
}

/// A c-string of a binary file that search-based replacement patches, see
/// [`find_cstring_occurrences`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CStringOccurrences {
    /// Absolute byte positions of the placeholder occurrences inside the c-string, never empty.
    pub offsets: Vec<usize>,
    /// Absolute position of the first byte of the NUL terminator, or the file size when the
    /// c-string is unterminated at end-of-file.
    pub nul_pos: usize,
    /// The encoding of the c-string and its placeholder occurrences.
    pub encoding: OffsetEncoding,
}

/// One placeholder occurrence in a text file.
struct TextPatch<'a> {
    /// Absolute byte position of the occurrence.
    offset: usize,
    prefix: &'a EncodedPrefix,
}

/// One c-string of a binary file that contains placeholder occurrences.
struct CStringPatch<'a> {
    /// Absolute byte positions of the occurrences inside this c-string.
    offsets: Cow<'a, [usize]>,
    /// Absolute position of the first byte of the NUL terminator, or the file size when the
    /// c-string is unterminated at end-of-file.
    nul_pos: usize,
    prefix: &'a EncodedPrefix,
}

/// Given the contents of a file copy it to the `destination` and in the process replace the
/// `prefix_placeholder` text with the `target_prefix` text, using the offsets recorded in
/// `paths.json` instead of searching the file contents.
///
/// Per the [draft CEP], an installer applies exactly the groups whose encodings its own
/// search-based replacement covers, so both paths produce the same bytes. rattler's search-based
/// replacement covers every encoding the draft CEP defines, so every recorded group is spliced.
/// Offsets with no groups at all mean there is nothing to splice: the file is copied through
/// unchanged apart from the shebang handling of text files.
///
/// Returns [`OffsetReplaceError::InconsistentMetadata`] (having written nothing) when the offsets
/// were recorded for a different `file_mode` or do not match the file, so the caller can fall
/// back to search-based replacement.
///
/// Nothing is searched: the only bytes inspected are the ones the metadata points at, namely the
/// encoded placeholder at each recorded offset, the zero code unit at each recorded c-string
/// terminator, and, when the file starts with `#!`, the `shebang_length` bytes of the first line
/// that the installer's shebang rules transform. Everything else is copied without being looked
/// at, which is what lets a consumer compute the patched size from the metadata alone.
///
/// [draft CEP]: https://github.com/conda/ceps/pull/179
pub fn copy_and_replace_placeholders_with_offsets(
    source_bytes: &[u8],
    mut destination: impl Write,
    prefix_placeholder: &str,
    target_prefix: &str,
    target_platform: &Subdir,
    file_mode: FileMode,
    offsets: &PrefixOffsets,
) -> Result<(), OffsetReplaceError> {
    if offsets.file_mode() != file_mode {
        return Err(InconsistentOffsetsError::FileModeMismatch {
            recorded: offsets.file_mode(),
            installed: file_mode,
        }
        .into());
    }

    match file_mode {
        FileMode::Text => copy_and_replace_textual_placeholder_offsets(
            source_bytes,
            destination,
            prefix_placeholder,
            target_prefix,
            target_platform,
            offsets.groups(),
            offsets.shebang_length(),
        )?,
        // conda does not replace the prefix in the binary files on windows
        // DLLs are loaded quite differently anyways (there is no rpath, for example).
        FileMode::Binary if target_platform.is_windows() => {
            destination.write_all(source_bytes)?;
        }
        FileMode::Binary => copy_and_replace_cstring_placeholder_offsets(
            source_bytes,
            destination,
            prefix_placeholder,
            target_prefix,
            offsets.groups(),
        )?,
    }
    Ok(())
}

/// Writes `source[start..end]` to `destination`, returning an [`std::io::ErrorKind::InvalidData`]
/// error instead of panicking when the range is invalid (out of bounds or out of order).
///
/// The offsets driving the prefix replacement come from a package's `paths.json`, which is not
/// trusted input. A malformed or malicious entry (an offset past the end of the file, or offsets
/// that are not sorted/overlapping) must surface as a recoverable error rather than crash the
/// process (which for e.g. a FUSE/NFS mount would take down the whole mount).
fn write_replacement_range<W: Write>(
    destination: &mut W,
    source: &[u8],
    start: usize,
    end: usize,
) -> Result<(), std::io::Error> {
    let slice = source.get(start..end).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "invalid prefix replacement offsets: range {start}..{end} is out of bounds or out \
                 of order for content of length {}",
                source.len()
            ),
        )
    })?;
    destination.write_all(slice)
}
