//! Prefix replacement in text files, where a replacement changes the file length and the leading
//! shebang line is transformed by the installer's own rules.

use std::borrow::Cow;
use std::io::Write;

use once_cell::sync::Lazy;
use rattler_conda_types::Platform;
use rattler_conda_types::package::{OffsetGroup, OffsetRanges};
use regex::Regex;

use super::{
    EncodedPrefix, OffsetReplaceError, TextPatch, encoded_prefix_for, write_replacement_range,
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
fn is_valid_shebang_length(shebang: &str, platform: &Platform) -> bool {
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
    platform: &Platform,
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
    target_platform: &Platform,
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

/// Finds every placeholder occurrence that starts at or after `from`, under every encoding,
/// ordered by position in the file.
///
/// Occurrences are the leftmost non-overlapping matches over the whole file, which is how the
/// draft CEP defines them, and how a producer records them; the ones before `from` are dropped
/// afterwards. Searching only `source_bytes[from..]` instead would find matches a producer's
/// file-wide scan consumed as part of an earlier, overlapping occurrence.
fn find_text_patches<'a>(
    source_bytes: &[u8],
    from: usize,
    prefixes: &'a [EncodedPrefix],
) -> Vec<TextPatch<'a>> {
    let mut patches = Vec::new();
    for prefix in prefixes {
        patches.extend(
            memchr::memmem::find_iter(source_bytes, &prefix.placeholder)
                .map(|offset| TextPatch { offset, prefix }),
        );
    }

    patches.sort_by_key(|patch| patch.offset);
    let mut end = 0;
    patches.retain(|patch| {
        let disjoint = patch.offset >= end;
        if disjoint {
            end = patch.offset + patch.prefix.placeholder.len();
        }
        disjoint
    });
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
/// region is handled separately: on targets with shebang handling ([`Platform::is_unix`]) the
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
    target_platform: &Platform,
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
) -> Result<usize, OffsetReplaceError> {
    if !source_bytes.starts_with(b"#!") {
        return if shebang_length.is_some() {
            Err(OffsetReplaceError::inconsistent(
                "shebang_length present but the file does not start with #!",
            ))
        } else {
            Ok(0)
        };
    }

    let len = shebang_length.ok_or_else(|| {
        OffsetReplaceError::inconsistent("file starts with #! but shebang_length is absent")
    })?;
    let region = source_bytes.get(..len).ok_or_else(|| {
        OffsetReplaceError::inconsistent(format!(
            "shebang_length {len} is past the end of the file ({} bytes)",
            source_bytes.len()
        ))
    })?;
    let ends_the_line = match memchr::memchr(b'\n', region) {
        Some(index) => index + 1 == len,
        None => len == source_bytes.len(),
    };
    if !ends_the_line {
        return Err(OffsetReplaceError::inconsistent(format!(
            "shebang_length {len} is not the length of the first line"
        )));
    }
    Ok(len)
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
    target_platform: &Platform,
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
) -> Result<Vec<TextPatch<'a>>, OffsetReplaceError> {
    let mut patches = Vec::new();
    for group in groups {
        let prefix = encoded_prefix_for(prefixes, &group.encoding)?;
        let OffsetRanges::Text(offsets) = &group.ranges else {
            return Err(OffsetReplaceError::inconsistent(
                "ranges shape does not match file mode",
            ));
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
) -> Result<(), OffsetReplaceError> {
    let mut prev_end = region_end;
    for patch in patches {
        let placeholder = patch.prefix.placeholder.as_slice();
        if patch.offset < region_end {
            return Err(OffsetReplaceError::inconsistent(format!(
                "offset {} lies inside the shebang region (< {region_end})",
                patch.offset
            )));
        }
        if patch.offset < prev_end {
            return Err(OffsetReplaceError::inconsistent(
                "offsets are not sorted in strictly increasing, non-overlapping order",
            ));
        }
        let end = patch
            .offset
            .checked_add(placeholder.len())
            .filter(|&end| end <= source_bytes.len())
            .ok_or_else(|| {
                OffsetReplaceError::inconsistent(format!(
                    "offset {} is out of range for content of length {}",
                    patch.offset,
                    source_bytes.len()
                ))
            })?;
        if &source_bytes[patch.offset..end] != placeholder {
            return Err(OffsetReplaceError::inconsistent(format!(
                "placeholder bytes are not present at recorded offset {}",
                patch.offset
            )));
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
