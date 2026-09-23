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

use rattler_conda_types::Platform;
use rattler_conda_types::package::{FileMode, OffsetEncoding, OffsetGroup, validate_offset_groups};

mod binary;
#[cfg(test)]
mod tests;
mod text;

pub use binary::{
    copy_and_replace_cstring_placeholder, copy_and_replace_cstring_placeholder_offsets,
};
pub use text::{
    copy_and_replace_textual_placeholder, copy_and_replace_textual_placeholder_offsets,
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
    target_platform: &Platform,
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
/// The `offsets` and `shebang_length` recorded in `paths.json` come from the
/// package producer and are not trusted. When they are inconsistent with the
/// file contents the install must not fail: the caller falls back to the
/// search-based replacement path. IO errors while writing the patched file
/// are surfaced separately.
///
/// The offset functions write nothing to the destination before returning
/// [`OffsetReplaceError::InconsistentMetadata`], so the caller can reuse the
/// same (still empty) destination for the fallback.
#[derive(Debug, thiserror::Error)]
pub enum OffsetReplaceError {
    /// The recorded `offsets`/`shebang_length` are inconsistent with the file
    /// contents. Callers should fall back to search-based replacement rather
    /// than failing the install.
    #[error("inconsistent prefix replacement metadata: {0}")]
    InconsistentMetadata(String),

    /// An IO error occurred while writing the patched file.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl OffsetReplaceError {
    fn inconsistent(msg: impl Into<String>) -> Self {
        OffsetReplaceError::InconsistentMetadata(msg.into())
    }
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
                let placeholder = encoding.encode(placeholder)?;
                let target = encoding.encode(target)?;
                (!placeholder.is_empty()).then_some(EncodedPrefix {
                    encoding,
                    placeholder,
                    target,
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
        self.encoding
            .code_unit_size()
            .expect("only encodings defined by the draft CEP are constructed")
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
    for prefix in prefixes {
        if prefix.shrinks_by().is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "target prefix cannot be longer than the placeholder prefix (encoding '{}')",
                    prefix.encoding.as_str()
                ),
            ));
        }
    }
    Ok(())
}

/// Looks up the encoded prefixes for the encoding of a recorded offset group.
fn encoded_prefix_for<'a>(
    prefixes: &'a [EncodedPrefix],
    encoding: &OffsetEncoding,
) -> Result<&'a EncodedPrefix, OffsetReplaceError> {
    prefixes
        .iter()
        .find(|prefix| &prefix.encoding == encoding)
        .ok_or_else(|| {
            OffsetReplaceError::inconsistent(format!(
                "there is no placeholder to replace under encoding '{}'",
                encoding.as_str()
            ))
        })
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
/// `prefix_placeholder` text with the `target_prefix` text, using the offset groups recorded in
/// `paths.json` instead of searching the file contents.
///
/// Per the [draft CEP], an installer applies exactly the groups whose encodings its own
/// search-based replacement covers, so both paths produce the same bytes. rattler's search-based
/// replacement covers every encoding the draft CEP defines, so every group of a valid list is
/// spliced. Valid metadata with no ranges at all means there is nothing to splice: the file is
/// copied through unchanged apart from the shebang handling of text files.
///
/// `shebang_length` bounds the leading shebang region for text files and is ignored for binary
/// files. Returns [`OffsetReplaceError::InconsistentMetadata`] (having written nothing) when the
/// metadata does not match the file, so the caller can fall back to search-based replacement.
///
/// Nothing is searched: the only bytes inspected are the ones the metadata points at, namely the
/// encoded placeholder at each recorded offset, the zero code unit at each recorded c-string
/// terminator, and, when the file starts with `#!`, the `shebang_length` bytes of the first line
/// that the installer's shebang rules transform. Everything else is copied without being looked
/// at, which is what lets a consumer compute the patched size from the metadata alone.
///
/// [draft CEP]: https://github.com/conda/ceps/pull/179
#[allow(clippy::too_many_arguments)]
pub fn copy_and_replace_placeholders_with_offsets(
    source_bytes: &[u8],
    mut destination: impl Write,
    prefix_placeholder: &str,
    target_prefix: &str,
    target_platform: &Platform,
    file_mode: FileMode,
    offsets: &[OffsetGroup],
    shebang_length: Option<usize>,
) -> Result<(), OffsetReplaceError> {
    validate_offset_groups(offsets, file_mode, shebang_length.is_some())
        .map_err(|err| OffsetReplaceError::inconsistent(err.to_string()))?;

    match file_mode {
        FileMode::Text => copy_and_replace_textual_placeholder_offsets(
            source_bytes,
            destination,
            prefix_placeholder,
            target_prefix,
            target_platform,
            offsets,
            shebang_length,
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
            offsets,
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
