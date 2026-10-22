//! Prefix replacement in text files, where a replacement changes the file length and the leading
//! shebang line is transformed by the installer's own rules.

use std::borrow::Cow;
use std::io::Write;

use once_cell::sync::Lazy;
use rattler_conda_types::Subdir;
use rattler_conda_types::package::{
    InvalidOffsetsError, OffsetEncoding, OffsetGroup, OffsetRanges,
};
use regex::Regex;

use super::{
    EncodedPrefix, InconsistentOffsetsError, OffsetReplaceError, TextOccurrence, TextPatch,
    encoded_prefix_for, write_replacement_range,
};

static SHEBANG_REGEX: Lazy<Regex> = Lazy::new(|| {
    // ^(#!      pretty much the whole match string
    // (?:[ ]*)  allow spaces between #! and beginning of
    //           the executable path
    // (/(?:\\ |[^ \n\r\t])*)  the executable is the next
    //                         text block without an
    //                         escaped space or non-space
    //                         whitespace character
    // (.*))$    the rest of the line can contain option
    //           flags and end whole_shebang group
    Regex::new(r"^(#!(?:[ ]*)(/(?:\\ |[^ \n\r\t])*)(.*))$").unwrap()
});

pub(super) static PYTHON_REGEX: Lazy<Regex> = Lazy::new(|| {
    // Match string starting with `python`, and optional version number
    // followed by optional flags.
    // python matches the string `python`
    // (?:\d+(?:\.\d+)*)? matches an optional version number
    Regex::new(r"^python(?:\d+(?:\.\d+)?)?$").unwrap()
});

/// Finds if the shebang line length is valid.
fn is_valid_shebang_length(shebang: &str, platform: &Subdir) -> bool {
    const MAX_SHEBANG_LENGTH_LINUX: usize = 127;
    const MAX_SHEBANG_LENGTH_MACOS: usize = 512;

    // Android uses the Linux kernel and therefore inherits its shebang limit;
    // iOS shares the XNU kernel with macOS.
    if platform.is_linux() || platform.is_android() {
        shebang.len() <= MAX_SHEBANG_LENGTH_LINUX
    } else if platform.is_osx() || platform.is_ios() {
        shebang.len() <= MAX_SHEBANG_LENGTH_MACOS
    } else {
        true
    }
}

/// Convert a shebang to use `/usr/bin/env` to find the executable.
/// This is useful for long shebangs or shebangs with spaces.
fn convert_shebang_to_env(shebang: Cow<'_, str>) -> Cow<'_, str> {
    if let Some(captures) = SHEBANG_REGEX.captures(&shebang) {
        let path = &captures[2];
        let exe_name = path.rsplit_once('/').map_or(path, |(_, f)| f);
        if PYTHON_REGEX.is_match(exe_name) {
            Cow::Owned(format!(
                "#!/bin/sh\n'''exec' \"{}\"{} \"$0\" \"$@\" #'''",
                path, &captures[3]
            ))
        } else {
            Cow::Owned(format!("#!/usr/bin/env {}{}", exe_name, &captures[3]))
        }
    } else {
        shebang
    }
}

/// Long shebangs and shebangs with spaces are invalid.
/// Long shebangs are longer than 127 on Linux or 512 on macOS characters.
/// Shebangs with spaces are replaced with a shebang that uses `/usr/bin/env` to find the executable.
/// This function replaces long shebangs with a shebang that uses `/usr/bin/env` to find the
/// executable.
pub(super) fn replace_shebang<'a>(
    shebang: Cow<'a, str>,
    old_new: (&str, &str),
    platform: &Subdir,
) -> Cow<'a, str> {
    // If the new shebang would contain a space, return a `#!/usr/bin/env` shebang
    assert!(
        shebang.starts_with("#!"),
        "Shebang does not start with #! ({shebang})",
    );

    if old_new.1.contains(' ') {
        // Doesn't matter if we don't replace anything
        if !shebang.contains(old_new.0) {
            return shebang;
        }
        // we convert the shebang without spaces to a new shebang, and only then replace
        // which is relevant for the Python case
        let new_shebang = convert_shebang_to_env(shebang).replace(old_new.0, old_new.1);
        return new_shebang.into();
    }

    let shebang: Cow<'_, str> = shebang.replace(old_new.0, old_new.1).into();

    if !shebang.starts_with("#!") {
        tracing::warn!("Shebang does not start with #! ({})", shebang);
        return shebang;
    }

    if is_valid_shebang_length(&shebang, platform) {
        shebang
    } else {
        convert_shebang_to_env(shebang)
    }
}

/// Given the contents of a file copy it to the `destination` and in the process replace the
/// `prefix_placeholder` text with the `target_prefix` text.
///
/// The placeholder is replaced under every encoding defined by the [draft CEP], so text stored as
/// wide characters is patched as well.
///
/// This is a text based version where the complete string is replaced. This works fine for text
/// files but will not work correctly for binary files where the length of the string is often
/// important. See [`super::copy_and_replace_cstring_placeholder`] when you are dealing with binary
/// content.
///
/// [draft CEP]: https://github.com/conda/ceps/pull/179
pub fn copy_and_replace_textual_placeholder(
    source_bytes: &[u8],
    mut destination: impl Write,
    prefix_placeholder: &str,
    target_prefix: &str,
    target_platform: &Subdir,
) -> Result<(), std::io::Error> {
    let prefixes = EncodedPrefix::all(prefix_placeholder, target_prefix);

    // Check if we have a shebang. We need to handle it differently because it has a maximum length
    // that can be exceeded in very long target prefix's. When the file has no newline the whole
    // file is the shebang line.
    let region_end = if target_platform.is_unix() && source_bytes.starts_with(b"#!") {
        source_bytes
            .iter()
            .position(|&c| c == b'\n')
            .map_or(source_bytes.len(), |index| index + 1)
    } else {
        0
    };
    write_shebang_region(
        &mut destination,
        source_bytes,
        region_end,
        prefix_placeholder,
        target_prefix,
        target_platform,
        &prefixes,
    )?;

    let patches = find_text_patches(source_bytes, region_end, &prefixes);
    write_patched_text(destination, source_bytes, region_end, &patches)
}

/// Finds the placeholder occurrences that search-based text replacement replaces, starting at or
/// after `from`, under every encoding the draft CEP defines and ordered by position in the file.
///
/// This exposes the search [`copy_and_replace_textual_placeholder`] performs, including how it
/// resolves overlapping matches of different encodings, so that callers patching files lazily,
/// such as the mount-time ranged reads in `rattler_vfs`, stay byte-identical with it. Pass the
/// end of the shebang region as `from` on targets that rewrite shebangs, as the installer does.
pub fn find_text_occurrences(
    source_bytes: &[u8],
    from: usize,
    prefix_placeholder: &str,
) -> Vec<TextOccurrence> {
    // Only the placeholder is searched for, so the target prefix does not matter here.
    let prefixes = EncodedPrefix::all(prefix_placeholder, "");
    find_text_patches(source_bytes, from, &prefixes)
        .into_iter()
        .map(|patch| TextOccurrence {
            offset: patch.offset,
            encoding: patch.prefix.encoding,
        })
        .collect()
}

/// Finds every placeholder occurrence that starts at or after `from`, under every encoding,
/// ordered by position in the file.
///
/// Per encoding, occurrences are the leftmost non-overlapping matches over the whole file, which
/// is how the draft CEP defines them; the ones before `from` are dropped afterwards. Searching only
/// `source_bytes[from..]` instead would find matches a producer's file-wide scan consumed as part
/// of an earlier, overlapping occurrence.
///
/// Matches of different encodings can overlap, because the encodings of an ASCII placeholder are
/// byte-shifted variants of one another: in UTF-16-LE text every occurrence has a spurious
/// UTF-16-BE twin one byte earlier. Of two overlapping matches the one preferred is the UTF-8
/// match, then the one whose offset is code-unit aligned (as every character of a wide-encoded
/// text file is), then the earlier one.
fn find_text_patches<'a>(
    source_bytes: &[u8],
    from: usize,
    prefixes: &'a [EncodedPrefix],
) -> Vec<TextPatch<'a>> {
    let mut candidates = Vec::new();
    for prefix in prefixes {
        candidates.extend(
            memchr::memmem::find_iter(source_bytes, &prefix.placeholder)
                .map(|offset| TextPatch { offset, prefix }),
        );
    }
    candidates.sort_by_key(|patch| patch.offset);

    let rank = |patch: &TextPatch<'_>| match patch.prefix.encoding {
        OffsetEncoding::Utf8 => 0u8,
        OffsetEncoding::Utf16Le
        | OffsetEncoding::Utf16Be
        | OffsetEncoding::Utf32Le
        | OffsetEncoding::Utf32Be => {
            if patch.offset.is_multiple_of(patch.prefix.code_unit_size()) {
                1
            } else {
                2
            }
        }
    };
    let mut patches: Vec<TextPatch<'a>> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        // The kept patches are disjoint and sorted, and the candidate starts at or after the last
        // one, so only the last one can overlap it.
        match patches.last() {
            Some(last) if candidate.offset < last.offset + last.prefix.placeholder.len() => {
                if rank(&candidate) < rank(last) {
                    patches.pop();
                    patches.push(candidate);
                }
            }
            Some(_) | None => patches.push(candidate),
        }
    }
    patches.retain(|patch| patch.offset >= from);
    patches
}

/// Given the contents of a file copy it to the `destination` and in the process replace the
/// `prefix_placeholder` text with the `target_prefix` text using the offsets from the `paths.json`.
///
/// This is a text based version where the complete string is replaced. This works fine for text
/// files but will not work correctly for binary files where the length of the string is often
/// important. See [`super::copy_and_replace_cstring_placeholder_offsets`] when you are dealing with binary
/// content.
///
/// Every group of `groups` is applied, each under its own encoding. Its ranges are absolute byte
/// positions in `source_bytes` and, per the draft CEP, exclude any occurrence inside the shebang
/// region (the first `shebang_length` bytes, present exactly when the file starts with `#!`). The
/// region is handled separately: on targets with shebang handling ([`Subdir::is_unix`]) the
/// region minus its trailing newline is rewritten by `replace_shebang` and the newline byte copied
/// through verbatim; on other targets the region gets plain placeholder replacement.
///
/// The recorded metadata is validated before anything is written, so a mismatch surfaces as
/// [`OffsetReplaceError::InconsistentMetadata`] with an untouched destination the caller can hand
/// to search-based replacement.
pub fn copy_and_replace_textual_placeholder_offsets(
    source_bytes: &[u8],
    mut destination: impl Write,
    prefix_placeholder: &str,
    target_prefix: &str,
    target_platform: &Subdir,
    groups: &[OffsetGroup],
    shebang_length: Option<usize>,
) -> Result<(), OffsetReplaceError> {
    let prefixes = EncodedPrefix::all(prefix_placeholder, target_prefix);
    let region_end = validated_shebang_region_end(source_bytes, shebang_length)?;
    let patches = text_patches_from_groups(groups, &prefixes)?;
    validate_text_patches(source_bytes, &patches, region_end)?;

    // --- The metadata is consistent; write the patched file. ---
    write_shebang_region(
        &mut destination,
        source_bytes,
        region_end,
        prefix_placeholder,
        target_prefix,
        target_platform,
        &prefixes,
    )?;
    write_patched_text(destination, source_bytes, region_end, &patches)?;
    Ok(())
}

/// Determines the shebang region from the recorded `shebang_length` rather than re-deriving it
/// from the file contents, validating the recorded value against those contents.
///
/// Per the draft CEP `shebang_length` is present exactly when the file starts with `#!`, and its
/// value is the offset of the first newline plus one, or the file size when there is no newline.
/// The first `shebang_length` bytes form the shebang region; a file without a shebang has an empty
/// one.
///
/// Validating that reads the region and nothing beyond it, which is the bound the draft CEP
/// promises consumers: the region is the first line, so its only newline is its last byte, unless
/// it runs to end-of-file.
fn validated_shebang_region_end(
    source_bytes: &[u8],
    shebang_length: Option<usize>,
) -> Result<usize, InconsistentOffsetsError> {
    if !source_bytes.starts_with(b"#!") {
        return match shebang_length {
            Some(_) => Err(InconsistentOffsetsError::UnexpectedShebangLength),
            None => Ok(0),
        };
    }

    let shebang_length = shebang_length.ok_or(InconsistentOffsetsError::MissingShebangLength)?;
    let region = source_bytes
        .get(..shebang_length)
        .ok_or(InconsistentOffsetsError::ShebangLengthMismatch { shebang_length })?;
    let ends_the_line = match memchr::memchr(b'\n', region) {
        Some(index) => index + 1 == shebang_length,
        None => shebang_length == source_bytes.len(),
    };
    if !ends_the_line {
        return Err(InconsistentOffsetsError::ShebangLengthMismatch { shebang_length });
    }
    Ok(shebang_length)
}

/// Transforms the shebang region (the first `shebang_length` bytes of a text file) exactly as the
/// installer does when writing the patched file, returning the region's contribution to the output.
///
/// On targets with shebang handling ([`Subdir::is_unix`]) the region minus its trailing newline is
/// rewritten by `replace_shebang` (which may collapse an over-long line to the
/// `#!/usr/bin/env <program>` form) and the trailing newline, if present, is appended unchanged. On
/// other targets the region receives plain placeholder replacement under every encoding.
///
/// This shares its implementation with install-time replacement so that callers patching files
/// lazily, such as the mount-time ranged reads in `rattler_vfs`, stay byte-identical with it.
pub fn replace_shebang_region(
    region: &[u8],
    prefix_placeholder: &str,
    target_prefix: &str,
    target_platform: &Subdir,
) -> Vec<u8> {
    let prefixes = EncodedPrefix::all(prefix_placeholder, target_prefix);
    let mut out = Vec::with_capacity(region.len());
    write_shebang_region(
        &mut out,
        region,
        region.len(),
        prefix_placeholder,
        target_prefix,
        target_platform,
        &prefixes,
    )
    .expect("writing to a Vec cannot fail");
    out
}

/// Writes the first `region_end` bytes of a text file, transformed by the installer's shebang
/// rules on targets that rewrite shebangs and by plain placeholder replacement everywhere else.
///
/// The plain replacement covers every encoding, exactly as the body splice does, so that a
/// wide-encoded occurrence in the first line is not left behind on a target without shebang
/// handling. An empty placeholder means there is nothing to replace: `prefixes` is then empty and
/// the region is copied verbatim rather than fed to a search that matches at every byte.
fn write_shebang_region(
    destination: &mut impl Write,
    source_bytes: &[u8],
    region_end: usize,
    prefix_placeholder: &str,
    target_prefix: &str,
    target_platform: &Subdir,
    prefixes: &[EncodedPrefix],
) -> Result<(), std::io::Error> {
    if region_end == 0 {
        return Ok(());
    }

    if prefixes.is_empty() {
        destination.write_all(&source_bytes[..region_end])?;
    } else if target_platform.is_unix() {
        // Feed the region minus its trailing newline to the shebang rules; the newline byte, when
        // present, is copied through unchanged.
        let has_newline = source_bytes[region_end - 1] == b'\n';
        let line_end = if has_newline {
            region_end - 1
        } else {
            region_end
        };
        let first_line = String::from_utf8_lossy(&source_bytes[..line_end]);
        let new_shebang = replace_shebang(
            first_line,
            (prefix_placeholder, target_prefix),
            target_platform,
        );
        destination.write_all(new_shebang.as_bytes())?;
        if has_newline {
            destination.write_all(&source_bytes[line_end..region_end])?;
        }
    } else {
        // On non-rewriting targets (e.g. Windows for a noarch package) the region gets plain
        // placeholder replacement, exactly as the body does, searching at most the first
        // `shebang_length` bytes.
        let region = &source_bytes[..region_end];
        let patches = find_text_patches(region, 0, prefixes);
        write_patched_text(&mut *destination, region, 0, &patches)?;
    }

    Ok(())
}

/// Collects the occurrences to replace from the recorded offset groups, ordered by their position
/// in the file so that the splice runs in file order regardless of which group a range came from.
fn text_patches_from_groups<'a>(
    groups: &[OffsetGroup],
    prefixes: &'a [EncodedPrefix],
) -> Result<Vec<TextPatch<'a>>, InconsistentOffsetsError> {
    let mut patches = Vec::new();
    for group in groups {
        let prefix = encoded_prefix_for(prefixes, group.encoding())?;
        let OffsetRanges::Text(offsets) = group.ranges() else {
            return Err(InvalidOffsetsError::RangesShapeMismatch(group.encoding()).into());
        };
        patches.extend(offsets.iter().map(|&offset| TextPatch { offset, prefix }));
    }
    patches.sort_by_key(|patch| patch.offset);
    Ok(patches)
}

/// Validates the occurrences to replace against the file contents.
///
/// Nothing may be written before this passes so that, on inconsistent metadata, the caller can
/// fall back to search-based replacement using the still-empty destination. Offsets must be in
/// range, sorted in strictly increasing non-overlapping order (also across encodings), at or after
/// the shebang region, and the placeholder bytes must be present at each one.
fn validate_text_patches(
    source_bytes: &[u8],
    patches: &[TextPatch<'_>],
    region_end: usize,
) -> Result<(), InconsistentOffsetsError> {
    let mut prev_end = region_end;
    for patch in patches {
        let offset = patch.offset;
        let placeholder = patch.prefix.placeholder.as_slice();
        if offset < region_end {
            return Err(InconsistentOffsetsError::OffsetInShebangRegion {
                offset,
                shebang_length: region_end,
            });
        }
        if offset < prev_end {
            return Err(InconsistentOffsetsError::UnsortedOffset { offset });
        }
        let end = offset
            .checked_add(placeholder.len())
            .filter(|&end| end <= source_bytes.len())
            .ok_or(InconsistentOffsetsError::OffsetOutOfRange {
                offset,
                file_size: source_bytes.len(),
            })?;
        if &source_bytes[offset..end] != placeholder {
            return Err(InconsistentOffsetsError::PlaceholderNotFound { offset });
        }
        prev_end = end;
    }
    Ok(())
}

/// Writes `source_bytes` from `start` onwards to `destination`, replacing the placeholder at every
/// patched position with the target prefix encoded the same way.
fn write_patched_text(
    mut destination: impl Write,
    source_bytes: &[u8],
    start: usize,
    patches: &[TextPatch<'_>],
) -> Result<(), std::io::Error> {
    let mut last_match = start;
    for patch in patches {
        write_replacement_range(&mut destination, source_bytes, last_match, patch.offset)?;
        destination.write_all(&patch.prefix.target)?;
        last_match = patch.offset + patch.prefix.placeholder.len();
    }

    // Write any remaining bytes after the final replacement.
    if last_match < source_bytes.len() {
        destination.write_all(&source_bytes[last_match..])?;
    }

    Ok(())
}
