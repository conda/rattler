//! Ranged prefix replacement for FUSE/NFS reads.
//!
//! Serves a byte range `[start, end)` from a source file with prefix
//! placeholder replacements applied on the fly, without materializing the
//! entire transformed file in memory.
//!
//! The placeholder is replaced under every encoding the draft CEP defines
//! (UTF-8, UTF-16 and UTF-32 in both byte orders), exactly as
//! `rattler::install::link` does: the occurrences come from the installer's
//! own search ([`find_text_occurrences`], [`find_cstring_occurrences`]) or
//! from the offsets recorded in `paths.json`, and each one is spliced with
//! the target prefix encoded the same way.

use std::cell::OnceCell;

use rattler::install::link::{
    CStringOccurrences, find_cstring_occurrences, find_text_occurrences, replace_shebang_region,
};
use rattler_conda_types::Subdir;
use rattler_conda_types::package::{OffsetEncoding, OffsetGroup, OffsetRanges};

/// The placeholder and target prefix under every encoding, encoded on first
/// use.
///
/// UTF-8 borrows the strings; the wide encodings are only encoded when a plan
/// actually contains an occurrence in them, so files without wide strings pay
/// nothing.
#[derive(Debug)]
pub struct EncodedPrefixes<'a> {
    placeholder: &'a str,
    target: &'a str,
    /// UTF-16-LE, UTF-16-BE, UTF-32-LE and UTF-32-BE, in that order.
    wide: [OnceCell<(Vec<u8>, Vec<u8>)>; 4],
}

impl<'a> EncodedPrefixes<'a> {
    /// Creates the table for replacing `placeholder` with `target`.
    pub fn new(placeholder: &'a str, target: &'a str) -> Self {
        Self {
            placeholder,
            target,
            wide: Default::default(),
        }
    }

    /// The placeholder and the target prefix encoded with `encoding`.
    pub fn get(&self, encoding: OffsetEncoding) -> (&[u8], &[u8]) {
        let index = match encoding {
            OffsetEncoding::Utf8 => return (self.placeholder.as_bytes(), self.target.as_bytes()),
            OffsetEncoding::Utf16Le => 0,
            OffsetEncoding::Utf16Be => 1,
            OffsetEncoding::Utf32Le => 2,
            OffsetEncoding::Utf32Be => 3,
        };
        let (placeholder, target) = self.wide[index].get_or_init(|| {
            (
                encoding.encode(self.placeholder),
                encoding.encode(self.target),
            )
        });
        (placeholder, target)
    }

    /// How many bytes one replacement under `encoding` adds (negative when the
    /// target prefix is shorter than the placeholder).
    fn delta(&self, encoding: OffsetEncoding) -> isize {
        let (placeholder, target) = self.get(encoding);
        target.len() as isize - placeholder.len() as isize
    }
}

/// One placeholder occurrence after the shebang region of a text file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextReplacement {
    /// Absolute source offset of the occurrence.
    pub offset: usize,
    /// The encoding the placeholder occurs in.
    pub encoding: OffsetEncoding,
    /// The total change in length caused by this replacement and every one
    /// before it, which maps source positions after it to output positions
    /// without walking the earlier replacements.
    pub delta_after: isize,
}

/// A precomputed, CEP-conformant text replacement plan for a single file.
///
/// Mirrors how `rattler::install::link` patches a text file: the shebang region
/// (the first line of a file starting with `#!`) is transformed by the
/// installer's shebang rules, and every remaining placeholder occurrence is
/// recorded with its encoding. Keeping this in one place guarantees mount-time
/// reads stay byte-identical to an install.
///
/// The plan is built for one target prefix: [`TextReplacement::delta_after`]
/// depends on its length, so reads must use the same target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextPlan {
    /// The occurrences after the shebang region, in file order (each spliced
    /// as a plain replacement under its own encoding).
    pub replacements: Vec<TextReplacement>,
    /// Length of the shebang region in the source, i.e. the boundary after
    /// which `replacements` apply. `0` when the file has no shebang region.
    pub region_end: usize,
    /// The already-transformed shebang region bytes (empty when no shebang).
    pub transformed_region: Vec<u8>,
}

/// Build the CEP-conformant text replacement plan for `source`.
///
/// On targets that rewrite shebangs ([`Subdir::is_unix`]) a file starting with
/// `#!` has a shebang region (up to and including the first newline, or the
/// whole file when there is none), transformed via the shared
/// [`replace_shebang_region`] helper — the same code the installer uses, so an
/// over-long line collapses to `#!/usr/bin/env <program>` exactly as it would
/// on install. The occurrences come from the installer's own search, which
/// excludes the region, covers every encoding and resolves overlapping matches
/// of different encodings the same way.
pub fn plan_text_replacement(
    source: &[u8],
    placeholder: &str,
    target: &str,
    platform: &Subdir,
) -> TextPlan {
    let region_end = if platform.is_unix() && source.starts_with(b"#!") {
        source
            .iter()
            .position(|&c| c == b'\n')
            .map_or(source.len(), |i| i + 1)
    } else {
        0
    };

    let transformed_region = if region_end > 0 {
        replace_shebang_region(&source[..region_end], placeholder, target, platform)
    } else {
        Vec::new()
    };

    let occurrences = find_text_occurrences(source, region_end, placeholder)
        .into_iter()
        .map(|occurrence| (occurrence.offset, occurrence.encoding));
    TextPlan::new(
        occurrences,
        region_end,
        transformed_region,
        &EncodedPrefixes::new(placeholder, target),
    )
}

impl TextPlan {
    fn new(
        occurrences: impl IntoIterator<Item = (usize, OffsetEncoding)>,
        region_end: usize,
        transformed_region: Vec<u8>,
        prefixes: &EncodedPrefixes<'_>,
    ) -> Self {
        let mut delta_after = 0isize;
        let replacements = occurrences
            .into_iter()
            .map(|(offset, encoding)| {
                delta_after += prefixes.delta(encoding);
                TextReplacement {
                    offset,
                    encoding,
                    delta_after,
                }
            })
            .collect();
        TextPlan {
            replacements,
            region_end,
            transformed_region,
        }
    }

    /// Build a plan from the metadata recorded in `paths.json` instead of
    /// scanning the file: the occurrences come straight from the recorded
    /// offset `groups` (every encoding, merged in file order), and `region`
    /// must hold the file's shebang region (its first `shebang_length` bytes)
    /// or be empty when no shebang is recorded. Only that region is ever read
    /// from disk — recorded offsets exist precisely so consumers don't have to
    /// scan file contents.
    ///
    /// The offsets are trusted as-is: per the CEP they are the producer's
    /// contract, and the ranged reads are total functions, so a
    /// non-conformant producer yields wrong bytes for its own package rather
    /// than a panic. The one sanity check is that a non-empty region starts
    /// with `#!` — the shared shebang transform requires it — and `None` is
    /// returned otherwise so the caller can fall back to
    /// [`plan_text_replacement`].
    pub fn from_recorded(
        region: &[u8],
        groups: &[OffsetGroup],
        placeholder: &str,
        target: &str,
        platform: &Subdir,
    ) -> Option<TextPlan> {
        let transformed_region = if region.is_empty() {
            Vec::new()
        } else if region.starts_with(b"#!") {
            replace_shebang_region(region, placeholder, target, platform)
        } else {
            return None;
        };

        let mut occurrences: Vec<(usize, OffsetEncoding)> = groups
            .iter()
            .filter_map(|group| match group.ranges() {
                OffsetRanges::Text(offsets) => Some((group.encoding(), offsets)),
                OffsetRanges::Binary(_) => None,
            })
            .flat_map(|(encoding, offsets)| offsets.iter().map(move |&offset| (offset, encoding)))
            .collect();
        occurrences.sort_by_key(|&(offset, _)| offset);

        Some(TextPlan::new(
            occurrences,
            region.len(),
            transformed_region,
            &EncodedPrefixes::new(placeholder, target),
        ))
    }

    /// The length of the patched file for a source of `source_len` bytes: the
    /// transformed shebang region, the unchanged body and the change in length
    /// of every replacement.
    pub fn output_len(&self, source_len: usize) -> usize {
        let total_delta = self.replacements.last().map_or(0, |r| r.delta_after);
        (self.transformed_region.len() as isize
            + source_len.saturating_sub(self.region_end) as isize
            + total_delta)
            .max(0) as usize
    }
}

/// Read a range from a text file with prefix replacements applied.
///
/// The transformed output is the already-transformed shebang region (see
/// [`TextPlan`]) followed by the body — `source[region_end..]` with each
/// occurrence replaced by the target prefix in the occurrence's encoding.
/// Because replacement can change length, output positions shift relative to
/// source positions.
///
/// Seeks directly to the window: a binary search over the replacements finds
/// the first one overlapping `start` and only the overlapping chunks are
/// copied, so the cost is `O(log k + (end - start))` rather than a walk of the
/// file. Offsets that don't match the file (out of range, unsorted) yield
/// best-effort bytes, never a panic — a panic on a FUSE/NFS read thread would
/// take down the whole mount.
///
/// `prefixes` must hold the placeholder and target the plan was built for.
/// Returns the bytes in the output range `[start, end)`.
pub fn text_ranged_read(
    source: &[u8],
    prefixes: &EncodedPrefixes<'_>,
    plan: &TextPlan,
    start: usize,
    end: usize,
) -> Vec<u8> {
    let replacements = plan.replacements.as_slice();
    let region_end = plan.region_end;
    let transformed_len = plan.output_len(source.len());

    let actual_end = end.min(transformed_len);
    let actual_start = start.min(transformed_len);
    if actual_start >= actual_end {
        return vec![];
    }

    let mut buffer = Vec::with_capacity(actual_end - actual_start);
    let region_out = plan.transformed_region.len();

    // 1. The already-transformed shebang region.
    emit_chunk(
        &plan.transformed_region,
        0,
        actual_start,
        actual_end,
        &mut buffer,
    );

    // Source position where the literal segment before replacement `j`
    // starts, and its output position: the `j` replacements before it shifted
    // the output by the `delta_after` of the last of them.
    let seg_src = |j: usize| -> usize {
        if j == 0 {
            region_end
        } else {
            let previous = &replacements[j - 1];
            previous
                .offset
                .saturating_add(prefixes.get(previous.encoding).0.len())
        }
    };
    let seg_out = |j: usize| -> usize {
        let shift = if j == 0 {
            0
        } else {
            replacements[j - 1].delta_after
        };
        (region_out as isize + seg_src(j).saturating_sub(region_end) as isize + shift).max(0)
            as usize
    };

    // 2. Seek: skip replacements whose output ends at or before the window.
    // `seg_out(j + 1)` is the output position just after replacement `j`.
    let mut lo = 0usize;
    let mut hi = replacements.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if seg_out(mid + 1) <= actual_start {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let j0 = lo;

    // 3. Emit the chunks overlapping the window from there.
    let mut src_pos = seg_src(j0);
    let mut out_pos = seg_out(j0);
    for replacement in &replacements[j0..] {
        if out_pos >= actual_end {
            break;
        }
        let (old_prefix, new_prefix) = prefixes.get(replacement.encoding);
        // Literal bytes before this replacement, then the replacement itself.
        out_pos = emit_source_range(
            source,
            src_pos,
            replacement.offset,
            out_pos,
            actual_start,
            actual_end,
            &mut buffer,
        );
        out_pos = emit_chunk(new_prefix, out_pos, actual_start, actual_end, &mut buffer);
        src_pos = replacement.offset.saturating_add(old_prefix.len());
    }

    // 4. The tail after the last replacement.
    if out_pos < actual_end {
        emit_source_range(
            source,
            src_pos,
            source.len(),
            out_pos,
            actual_start,
            actual_end,
            &mut buffer,
        );
    }

    buffer
}

/// Find the c-strings of a binary file to patch by scanning `source`, exactly
/// as the installer's search-based replacement does (every encoding, with the
/// same resolution of overlapping candidates).
pub fn plan_binary_replacement(source: &[u8], placeholder: &str) -> Vec<CStringOccurrences> {
    find_cstring_occurrences(source, placeholder)
}

/// The c-strings to patch as recorded in `paths.json`: every binary offset
/// group, merged in file order. Each recorded c-string lists its prefix
/// offsets followed by the NUL terminator position.
///
/// Like [`TextPlan::from_recorded`] the offsets are trusted as-is; the ranged
/// reads are total, so metadata that doesn't match the file yields wrong
/// bytes, never a panic.
pub fn cstrings_from_recorded(groups: &[OffsetGroup]) -> Vec<CStringOccurrences> {
    let mut cstrings: Vec<CStringOccurrences> = groups
        .iter()
        .filter_map(|group| match group.ranges() {
            OffsetRanges::Binary(cstrings) => Some((group.encoding(), cstrings)),
            OffsetRanges::Text(_) => None,
        })
        .flat_map(|(encoding, cstrings)| {
            cstrings.iter().filter_map(move |cstring| {
                let (&nul_pos, offsets) = cstring.split_last()?;
                Some(CStringOccurrences {
                    offsets: offsets.to_vec(),
                    nul_pos,
                    encoding,
                })
            })
        })
        .collect();
    cstrings.sort_by_key(|cstring| cstring.offsets.first().copied().unwrap_or(cstring.nul_pos));
    cstrings
}

/// Read a range from a binary file with prefix replacements applied.
///
/// Binary-mode replacement swaps the placeholder for the target prefix inside
/// each c-string, in the c-string's encoding, and pads with zero bytes before
/// its NUL terminator to maintain the same total length.
///
/// Output length always equals source length. Because every c-string repays
/// its replacements' byte deficit with padding, source and output positions
/// coincide at each c-string boundary; seeking therefore skips whole c-strings
/// with a binary search and copies only the chunks overlapping the window:
/// `O(log g + (end - start))`. C-strings that don't match the file (empty, out
/// of range, unsorted) yield best-effort bytes, never a panic — a panic on a
/// FUSE/NFS read thread would take down the whole mount. A c-string whose
/// encoded target prefix is longer than the placeholder cannot be patched in
/// place (the installer refuses such an install) and is served unchanged.
///
/// Returns the bytes in the output range `[start, end)`.
pub fn binary_ranged_read(
    source: &[u8],
    prefixes: &EncodedPrefixes<'_>,
    cstrings: &[CStringOccurrences],
    start: usize,
    end: usize,
) -> Vec<u8> {
    let src_len = source.len();
    let actual_end = end.min(src_len);
    let actual_start = start.min(src_len);
    if actual_start >= actual_end {
        return vec![];
    }

    let mut buffer = Vec::with_capacity(actual_end - actual_start);

    // Seek: skip c-strings that end at or before the window start (output and
    // source positions agree at c-string boundaries, so the comparison is
    // exact).
    let g0 = cstrings.partition_point(|cstring| cstring.nul_pos.min(src_len) <= actual_start);

    let mut src_pos = if g0 == 0 {
        0
    } else {
        cstrings[g0 - 1].nul_pos.min(src_len)
    };
    let mut out_pos = src_pos;

    for cstring in &cstrings[g0..] {
        if out_pos >= actual_end {
            break;
        }
        let (old_prefix, new_prefix) = prefixes.get(cstring.encoding);
        // `None` when the target does not fit: the c-string is copied as-is.
        let length_change = old_prefix.len().checked_sub(new_prefix.len());

        if let Some(length_change) = length_change {
            for &offset in &cstring.offsets {
                // Literal bytes before this prefix, then the replacement.
                out_pos = emit_source_range(
                    source,
                    src_pos,
                    offset,
                    out_pos,
                    actual_start,
                    actual_end,
                    &mut buffer,
                );
                out_pos = emit_chunk(new_prefix, out_pos, actual_start, actual_end, &mut buffer);
                src_pos = offset.saturating_add(old_prefix.len());
            }

            // Bytes from the last prefix end to the NUL, then the zero
            // padding that restores the c-string's original length.
            out_pos = emit_source_range(
                source,
                src_pos,
                cstring.nul_pos,
                out_pos,
                actual_start,
                actual_end,
                &mut buffer,
            );
            out_pos = emit_zeros(
                cstring.offsets.len().saturating_mul(length_change),
                out_pos,
                actual_start,
                actual_end,
                &mut buffer,
            );
        } else {
            out_pos = emit_source_range(
                source,
                src_pos,
                cstring.nul_pos,
                out_pos,
                actual_start,
                actual_end,
                &mut buffer,
            );
        }
        src_pos = cstring.nul_pos.min(src_len);
    }

    // Remaining source bytes after the last c-string.
    if out_pos < actual_end {
        emit_source_range(
            source,
            src_pos,
            src_len,
            out_pos,
            actual_start,
            actual_end,
            &mut buffer,
        );
    }

    buffer
}

/// Append the overlap of `chunk` (whose output span begins at `out_pos`) with
/// the window `[win_start, win_end)` to `buffer`. Returns the output position
/// just after the chunk.
#[inline]
fn emit_chunk(
    chunk: &[u8],
    out_pos: usize,
    win_start: usize,
    win_end: usize,
    buffer: &mut Vec<u8>,
) -> usize {
    let chunk_end = out_pos.saturating_add(chunk.len());
    if chunk_end > win_start && out_pos < win_end {
        let from = win_start.saturating_sub(out_pos);
        let to = chunk.len() - chunk_end.saturating_sub(win_end);
        buffer.extend_from_slice(&chunk[from..to]);
    }
    chunk_end
}

/// [`emit_chunk`] for `source[src_start..src_end]`, clamping the range to the
/// source bounds so offsets that don't match the file cannot panic a read.
#[inline]
fn emit_source_range(
    source: &[u8],
    src_start: usize,
    src_end: usize,
    out_pos: usize,
    win_start: usize,
    win_end: usize,
    buffer: &mut Vec<u8>,
) -> usize {
    let from = src_start.min(source.len());
    let to = src_end.clamp(from, source.len());
    emit_chunk(&source[from..to], out_pos, win_start, win_end, buffer)
}

/// [`emit_chunk`] for a run of `count` zero bytes (binary-mode padding).
#[inline]
fn emit_zeros(
    count: usize,
    out_pos: usize,
    win_start: usize,
    win_end: usize,
    buffer: &mut Vec<u8>,
) -> usize {
    let chunk_end = out_pos.saturating_add(count);
    if chunk_end > win_start && out_pos < win_end {
        let n = chunk_end.min(win_end) - out_pos.max(win_start);
        buffer.resize(buffer.len() + n, 0);
    }
    chunk_end
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Text mode tests ──────────────────────────────────────────────

    use rattler_conda_types::Subdir;

    /// Plans `source` by scanning and reads `[start, end)` of the output.
    fn text_read(
        source: &[u8],
        placeholder: &str,
        target: &str,
        platform: Subdir,
        start: usize,
        end: usize,
    ) -> Vec<u8> {
        let plan = plan_text_replacement(source, placeholder, target, &platform);
        let prefixes = EncodedPrefixes::new(placeholder, target);
        text_ranged_read(source, &prefixes, &plan, start, end)
    }

    /// Plans `source` by scanning and reads `[start, end)` of the output.
    fn binary_read(
        source: &[u8],
        placeholder: &str,
        target: &str,
        start: usize,
        end: usize,
    ) -> Vec<u8> {
        let cstrings = plan_binary_replacement(source, placeholder);
        let prefixes = EncodedPrefixes::new(placeholder, target);
        binary_ranged_read(source, &prefixes, &cstrings, start, end)
    }

    /// A c-string in the recorded form: prefix offsets, then the terminator.
    fn cstring(encoding: OffsetEncoding, recorded: &[usize]) -> CStringOccurrences {
        let (&nul_pos, offsets) = recorded.split_last().unwrap_or((&0, &[]));
        CStringOccurrences {
            offsets: offsets.to_vec(),
            nul_pos,
            encoding,
        }
    }

    fn text_test(
        placeholder: &str,
        prefix: &str,
        source: &[u8],
        expected: &[u8],
        start: usize,
        end: usize,
    ) {
        // These cases have no shebang, so every occurrence is a body offset and
        // the transformed region is empty.
        let result = text_read(source, placeholder, prefix, Subdir::Linux64, start, end);
        assert_eq!(
            result, expected,
            "text replacement [{start}..{end}] of {source:?}: expected {expected:?}, got {result:?}"
        );
    }

    fn binary_test(
        placeholder: &str,
        prefix: &str,
        source: &[u8],
        expected: &[u8],
        start: usize,
        end: usize,
    ) {
        let result = binary_read(source, placeholder, prefix, start, end);
        assert_eq!(
            result, expected,
            "binary replacement [{start}..{end}] of {source:?}: expected {expected:?}, got {result:?}"
        );
    }

    // Full-file text replacements

    #[test]
    fn text_full_file() {
        text_test(
            "ABCD",
            "XY",
            b"01ABCD23456ABCD7890",
            b"01XY23456XY7890",
            0,
            19,
        );
    }

    #[test]
    fn text_only_placeholder() {
        text_test("ABCD", "XY", b"ABCD", b"XY", 0, 4);
    }

    #[test]
    fn text_consecutive_placeholders() {
        text_test("ABCD", "XY", b"ABCDABCD", b"XYXY", 0, 8);
    }

    #[test]
    fn text_no_placeholders() {
        text_test("ABCD", "XY", b"0123456789", b"0123456789", 0, 10);
    }

    #[test]
    fn text_same_length() {
        text_test(
            "ABCD",
            "WXYZ",
            b"01ABCD6789012ABCD7890",
            b"01WXYZ6789012WXYZ7890",
            0,
            21,
        );
    }

    #[test]
    fn text_empty_file() {
        text_test("ABCD", "XY", b"", b"", 0, 0);
    }

    #[test]
    fn text_many_placeholders() {
        let mut source = Vec::new();
        let mut expected = Vec::new();
        for i in 0..10u8 {
            source.extend_from_slice(&[i + b'0', i + b'0']);
            source.extend_from_slice(b"ABCD");
            expected.extend_from_slice(&[i + b'0', i + b'0']);
            expected.extend_from_slice(b"XY");
        }
        text_test("ABCD", "XY", &source, &expected, 0, source.len());
    }

    // Partial-range text replacements

    #[test]
    fn text_partial_range() {
        text_test("ABCD", "XY", b"ABCD0ABCD5ABCD0ABCD5ABCD", b"0XY5XY0", 2, 9);
    }

    #[test]
    fn text_start_after_prefix() {
        // Output: "XY01234XY56789" (14 chars) — start at index 5 = "34XY56789"
        text_test("ABCD", "XY", b"ABCD01234ABCD56789", b"34XY56789", 5, 18);
    }

    #[test]
    fn text_start_between_placeholders() {
        // Output: "XY0123XY5678XY" — start at index 5 = "3XY5678XY"
        text_test("ABCD", "XY", b"ABCD0123ABCD5678ABCD", b"3XY5678XY", 5, 20);
    }

    #[test]
    fn text_start_at_placeholder() {
        // Output: "01234XY6789XY" — start at index 5 = "XY6789XY"
        text_test("ABCD", "XY", b"01234ABCD6789ABCD", b"XY6789XY", 5, 17);
    }

    #[test]
    fn text_longer_placeholder() {
        text_test(
            "ABCDEFGH",
            "XYZ",
            b"01ABCDEFGH234ABCDEFGH567",
            b"01XYZ234XYZ567",
            0,
            24,
        );
    }

    // ── Binary mode tests ────────────────────────────────────────────

    #[test]
    fn binary_full_file() {
        binary_test(
            "ABCD",
            "XY",
            b"01ABCD23\x00456ABCD78\x0090",
            b"01XY23\x00\x00\x00456XY78\x00\x00\x0090",
            0,
            21,
        );
    }

    #[test]
    fn binary_only_placeholder() {
        binary_test("ABCD", "XY", b"ABCD", b"XY\x00\x00", 0, 4);
    }

    #[test]
    fn binary_no_placeholders() {
        binary_test("ABCD", "XY", b"0123456789", b"0123456789", 0, 10);
    }

    #[test]
    fn binary_same_length() {
        binary_test(
            "ABCD",
            "WXYZ",
            b"01ABCD6789012ABCD7890",
            b"01WXYZ6789012WXYZ7890",
            0,
            21,
        );
    }

    #[test]
    fn binary_consecutive_placeholders() {
        binary_test("ABCD", "XY", b"ABCDABCD", b"XYXY\x00\x00\x00\x00", 0, 8);
    }

    #[test]
    fn binary_multiple_placeholders() {
        let source = b"\x00\x00ABCDZ\x00\x00\x00ABCDEFABCDEF\x00\x00\x00ABCDMNOPQRSABCDMNOPQRSABCDMNOPQRS\x00\x00";
        let expected = b"\x00\x00XYZ\x00\x00\x00\x00\x00XYEFXYEF\x00\x00\x00\x00\x00\x00\x00XYMNOPQRSXYMNOPQRSXYMNOPQRS\x00\x00\x00\x00\x00\x00\x00\x00";
        binary_test("ABCD", "XY", source, expected, 0, source.len());
    }

    #[test]
    fn binary_empty_file() {
        binary_test("ABCD", "XY", b"", b"", 0, 0);
    }

    #[test]
    fn binary_many_placeholders() {
        let mut source = Vec::new();
        let mut expected = Vec::new();
        for i in 0..10u8 {
            source.extend_from_slice(&[i + b'0', i + b'0']);
            source.extend_from_slice(b"ABCD\x00");
            expected.extend_from_slice(&[i + b'0', i + b'0']);
            expected.extend_from_slice(b"XY\x00\x00\x00");
        }
        binary_test("ABCD", "XY", &source, &expected, 0, source.len());
    }

    // Partial-range binary replacements

    #[test]
    fn binary_partial_range() {
        binary_test(
            "ABCD",
            "XY",
            b"ABCD\x000ABCD\x005ABCD\x000ABCD\x005ABCD\x00",
            b"0XY\x00\x00",
            5,
            10,
        );
    }

    #[test]
    fn binary_start_after_prefix() {
        binary_test(
            "ABCD",
            "XY",
            b"ABCD01234ABCD\x0056789",
            b"34XY\x00\x00\x00\x00\x0056789",
            5,
            19,
        );
    }

    #[test]
    fn binary_start_between_placeholders() {
        binary_test(
            "ABCD",
            "XY",
            b"ABCD012\x003ABCD5678ABCD",
            b"012\x00\x00\x003XY5678XY\x00\x00\x00\x00",
            2,
            21,
        );
    }

    #[test]
    fn binary_start_at_placeholder() {
        binary_test(
            "ABCD",
            "XY",
            b"01234ABCD\x006789ABCD\x00",
            b"XY\x00\x00\x006789XY\x00\x00\x00",
            5,
            19,
        );
    }

    #[test]
    fn binary_longer_placeholder() {
        binary_test(
            "ABCDEFGH",
            "XYZ",
            b"01ABCDEFGH234ABCDEFGH567",
            b"01XYZ234XYZ567\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00",
            0,
            24,
        );
    }

    // ── Offset collection ────────────────────────────────────────────

    fn body_offsets(plan: &TextPlan) -> Vec<usize> {
        plan.replacements.iter().map(|r| r.offset).collect()
    }

    fn scan_text(source: &[u8], placeholder: &str) -> Vec<usize> {
        body_offsets(&plan_text_replacement(
            source,
            placeholder,
            "/x",
            &Subdir::Linux64,
        ))
    }

    fn utf8_cstrings(recorded: &[&[usize]]) -> Vec<CStringOccurrences> {
        recorded
            .iter()
            .map(|r| cstring(OffsetEncoding::Utf8, r))
            .collect()
    }

    #[test]
    fn collect_offsets_basic() {
        assert_eq!(scan_text(b"01ABCD56ABCD", "ABCD"), vec![2, 8]);
    }

    #[test]
    fn collect_offsets_none() {
        assert_eq!(scan_text(b"0123456789", "ABCD"), Vec::<usize>::new());
    }

    #[test]
    fn collect_offsets_consecutive() {
        assert_eq!(scan_text(b"ABCDABCD", "ABCD"), vec![0, 4]);
    }

    #[test]
    fn collect_binary_offsets_single() {
        // One prefix in one c-string
        assert_eq!(
            plan_binary_replacement(b"hello/PFX/bin\x00tail", "/PFX"),
            utf8_cstrings(&[&[5, 13]])
        );
    }

    #[test]
    fn collect_binary_offsets_multi_in_one_cstring() {
        // Two prefixes sharing one c-string (PATH-style)
        assert_eq!(
            plan_binary_replacement(b"PATH=/PFX/a:/PFX/b\x00tail", "/PFX"),
            utf8_cstrings(&[&[5, 12, 18]])
        );
    }

    #[test]
    fn collect_binary_offsets_separate_cstrings() {
        // Two prefixes in separate c-strings
        assert_eq!(
            plan_binary_replacement(b"/PFX/a\x00/PFX/b\x00", "/PFX"),
            utf8_cstrings(&[&[0, 6], &[7, 13]])
        );
    }

    // ── Shebang-aware text plans ─────────────────────────────────────

    #[test]
    fn plan_no_shebang() {
        let plan = plan_text_replacement(b"hello /PFX world", "/PFX", "/new", &Subdir::Linux64);
        assert_eq!(plan.region_end, 0);
        assert!(plan.transformed_region.is_empty());
        assert_eq!(body_offsets(&plan), vec![6]);
    }

    #[test]
    fn plan_shebang_excludes_region_occurrence() {
        // "#!/PFX/python\n" is 14 bytes; the region occurrence at offset 2 is
        // excluded, the body occurrence is kept, and the region is rewritten.
        let src = b"#!/PFX/python\nimport x  # /PFX/lib\n";
        let plan = plan_text_replacement(src, "/PFX", "/new", &Subdir::Linux64);
        assert_eq!(plan.region_end, 14);
        assert_eq!(body_offsets(&plan), vec![26]);
        assert_eq!(plan.transformed_region, b"#!/new/python\n");
    }

    #[test]
    fn shebang_full_read_matches() {
        let src = b"#!/PFX/python\nimport x  # /PFX/lib\n";
        let out = text_read(src, "/PFX", "/new", Subdir::Linux64, 0, 1000);
        assert_eq!(out, b"#!/new/python\nimport x  # /new/lib\n");
    }

    #[test]
    fn shebang_ranged_reads_cross_region_boundary() {
        let src = b"#!/PFX/python\nimport x  # /PFX/lib\n";
        let full: &[u8] = b"#!/new/python\nimport x  # /new/lib\n";
        for (s, e) in [(0usize, 5), (10, 20), (13, 15), (0, full.len()), (30, 100)] {
            let out = text_read(src, "/PFX", "/new", Subdir::Linux64, s, e);
            let exp = &full[s.min(full.len())..e.min(full.len())];
            assert_eq!(out, exp, "range [{s}, {e})");
        }
    }

    #[test]
    fn plan_shebang_no_trailing_newline() {
        // Whole file is the shebang line; region covers everything, no body.
        let src = b"#!/PFX/python";
        let plan = plan_text_replacement(src, "/PFX", "/new", &Subdir::Linux64);
        assert_eq!(plan.region_end, src.len());
        assert!(plan.replacements.is_empty());
        assert_eq!(plan.transformed_region, b"#!/new/python");
    }

    // ── Plans built from recorded paths.json metadata ────────────────

    #[test]
    fn from_recorded_matches_scan() {
        let src = b"#!/PFX/python\nimport x  # /PFX/lib\n";
        let scanned = plan_text_replacement(src, "/PFX", "/new", &Subdir::Linux64);
        let groups = [OffsetGroup::new(
            OffsetEncoding::Utf8,
            OffsetRanges::Text(body_offsets(&scanned)),
        )
        .unwrap()];
        let recorded = TextPlan::from_recorded(
            &src[..scanned.region_end],
            &groups,
            "/PFX",
            "/new",
            &Subdir::Linux64,
        )
        .unwrap();
        assert_eq!(recorded, scanned);
    }

    #[test]
    fn from_recorded_no_shebang() {
        let groups = [OffsetGroup::new(OffsetEncoding::Utf8, OffsetRanges::Text(vec![6])).unwrap()];
        let plan = TextPlan::from_recorded(b"", &groups, "/PFX", "/new", &Subdir::Linux64).unwrap();
        assert_eq!(plan.region_end, 0);
        assert!(plan.transformed_region.is_empty());
        assert_eq!(body_offsets(&plan), vec![6]);
    }

    #[test]
    fn from_recorded_rejects_non_shebang_region() {
        // Recorded shebang_length but the file doesn't start with `#!`:
        // the caller must fall back to scanning.
        assert!(
            TextPlan::from_recorded(b"not a shebang\n", &[], "/PFX", "/new", &Subdir::Linux64)
                .is_none()
        );
    }

    // ── Seek correctness: every window equals the same slice of the
    //    full output ────────────────────────────────────────────────

    #[test]
    fn text_windows_match_full_output() {
        let mut source = Vec::new();
        for i in 0..50u8 {
            source.extend_from_slice(format!("chunk{i:02}/PFX").as_bytes());
        }
        let expected = String::from_utf8(source.clone())
            .unwrap()
            .replace("/PFX", "/replacement")
            .into_bytes();
        let plan = plan_text_replacement(&source, "/PFX", "/replacement", &Subdir::Linux64);
        let prefixes = EncodedPrefixes::new("/PFX", "/replacement");
        for start in (0..expected.len()).step_by(7) {
            for len in [1usize, 3, 17, 64] {
                let end = start + len;
                let out = text_ranged_read(&source, &prefixes, &plan, start, end);
                assert_eq!(
                    out,
                    &expected[start..end.min(expected.len())],
                    "window [{start}, {end})"
                );
            }
        }
    }

    #[test]
    fn binary_windows_match_full_output() {
        let mut source = Vec::new();
        for i in 0..50u8 {
            source.extend_from_slice(format!("path{i:02}=/PFXDIR/x\0pad").as_bytes());
        }
        let full = binary_read(&source, "/PFXDIR", "/np", 0, source.len());
        assert_eq!(full.len(), source.len(), "binary mode preserves length");
        for start in (0..source.len()).step_by(11) {
            for len in [1usize, 5, 33] {
                let end = (start + len).min(source.len());
                let out = binary_read(&source, "/PFXDIR", "/np", start, end);
                assert_eq!(out, &full[start..end], "window [{start}, {end})");
            }
        }
    }

    // ── Robustness: recorded offsets are trusted, so metadata that
    //    doesn't match the file must yield bounded garbage, never a
    //    panic (a panic on a FUSE/NFS read thread kills the mount) ────

    #[test]
    fn binary_malformed_groups_do_not_panic() {
        let source = b"12345/PFX67890\x00tail";
        let cases: &[&[&[usize]]] = &[
            &[&[]],                       // empty group
            &[&[5, 1000]],                // NUL past EOF
            &[&[1000, 2000]],             // everything past EOF
            &[&[9, 5, 14]],               // unsorted prefixes
            &[&[5, 14], &[3, 8]],         // overlapping groups
            &[&[usize::MAX, usize::MAX]], // overflow bait
        ];
        let prefixes = EncodedPrefixes::new("/PFX", "/np");
        for recorded in cases {
            let groups = utf8_cstrings(recorded);
            for (s, e) in [(0usize, 100), (5, 10), (10, 5)] {
                let out = binary_ranged_read(source, &prefixes, &groups, s, e);
                assert!(out.len() <= source.len(), "groups {recorded:?}");
            }
        }
    }

    #[test]
    fn text_malformed_offsets_do_not_panic() {
        let source = b"12345/PFX67890";
        let cases: &[Vec<usize>] = &[vec![1000], vec![9, 2], vec![usize::MAX], vec![5, 6]];
        let prefixes = EncodedPrefixes::new("/PFX", "/replacement");
        for offsets in cases {
            let groups =
                [
                    OffsetGroup::new(OffsetEncoding::Utf8, OffsetRanges::Text(offsets.clone()))
                        .unwrap(),
                ];
            let plan =
                TextPlan::from_recorded(b"", &groups, "/PFX", "/replacement", &Subdir::Linux64)
                    .unwrap();
            for (s, e) in [(0usize, 100), (3, 8)] {
                let _ = text_ranged_read(source, &prefixes, &plan, s, e);
            }
        }
    }

    // ── Wide encodings (UTF-16 / UTF-32) ─────────────────────────────

    const WIDE: [OffsetEncoding; 4] = [
        OffsetEncoding::Utf16Le,
        OffsetEncoding::Utf16Be,
        OffsetEncoding::Utf32Le,
        OffsetEncoding::Utf32Be,
    ];

    #[test]
    fn encoded_prefixes_match_encode() {
        let prefixes = EncodedPrefixes::new("/PFX", "/new/prefix");
        for encoding in OffsetEncoding::DEFINED {
            let (placeholder, target) = prefixes.get(encoding);
            assert_eq!(placeholder, encoding.encode("/PFX"), "{encoding}");
            assert_eq!(target, encoding.encode("/new/prefix"), "{encoding}");
        }
    }

    #[test]
    fn text_wide_encoded_file() {
        // A whole text file stored in a wide encoding: every occurrence is
        // replaced in that encoding and the output grows accordingly.
        for encoding in WIDE {
            let source = encoding.encode("a=/PFX/lib\nb=/PFX/bin\n");
            let expected = encoding.encode("a=/a/longer/prefix/lib\nb=/a/longer/prefix/bin\n");
            let plan = plan_text_replacement(&source, "/PFX", "/a/longer/prefix", &Subdir::Linux64);
            assert_eq!(plan.replacements.len(), 2, "{encoding}");
            assert!(plan.replacements.iter().all(|r| r.encoding == encoding));
            assert_eq!(plan.output_len(source.len()), expected.len(), "{encoding}");
            let out = text_read(
                &source,
                "/PFX",
                "/a/longer/prefix",
                Subdir::Linux64,
                0,
                1 << 20,
            );
            assert_eq!(out, expected, "{encoding}");
        }
    }

    #[test]
    fn text_mixed_encodings_windows_match_full_output() {
        // UTF-8 and UTF-16-LE occurrences in one file shift the output by
        // different amounts; every window must still line up with the full
        // output.
        let mut source = Vec::new();
        let mut expected = Vec::new();
        for i in 0..20 {
            let utf8 = format!("u8 {i:02} /PFX;");
            source.extend_from_slice(utf8.as_bytes());
            expected.extend_from_slice(utf8.replace("/PFX", "/target/dir").as_bytes());
            // Keep the wide strings code-unit aligned, as compilers emit them.
            let pad = source.len().next_multiple_of(2) - source.len();
            source.extend(std::iter::repeat_n(b' ', pad));
            expected.extend(std::iter::repeat_n(b' ', pad));
            let wide = format!("w{i:02}=/PFX;");
            source.extend(OffsetEncoding::Utf16Le.encode(&wide));
            expected.extend(OffsetEncoding::Utf16Le.encode(&wide.replace("/PFX", "/target/dir")));
        }

        let plan = plan_text_replacement(&source, "/PFX", "/target/dir", &Subdir::Linux64);
        let encodings: Vec<_> = plan.replacements.iter().map(|r| r.encoding).collect();
        assert!(encodings.contains(&OffsetEncoding::Utf8));
        assert!(encodings.contains(&OffsetEncoding::Utf16Le));
        assert_eq!(plan.output_len(source.len()), expected.len());

        let prefixes = EncodedPrefixes::new("/PFX", "/target/dir");
        assert_eq!(
            text_ranged_read(&source, &prefixes, &plan, 0, expected.len()),
            expected
        );
        for start in (0..expected.len()).step_by(5) {
            for len in [1usize, 2, 7, 31, 100] {
                let end = start + len;
                let out = text_ranged_read(&source, &prefixes, &plan, start, end);
                assert_eq!(
                    out,
                    &expected[start..end.min(expected.len())],
                    "window [{start}, {end})"
                );
            }
        }
    }

    #[test]
    fn text_from_recorded_wide_groups_match_scan() {
        let mut source = b"#!/PFX/python\nx = '/PFX'\n".to_vec();
        // Wide strings are code-unit aligned, as compilers emit them.
        source.resize(source.len().next_multiple_of(4), b' ');
        source.extend(OffsetEncoding::Utf32Be.encode("/PFX/share"));
        let scanned = plan_text_replacement(&source, "/PFX", "/new", &Subdir::Linux64);

        let mut utf8 = Vec::new();
        let mut utf32 = Vec::new();
        for r in &scanned.replacements {
            match r.encoding {
                OffsetEncoding::Utf8 => utf8.push(r.offset),
                OffsetEncoding::Utf32Be => utf32.push(r.offset),
                other => panic!("unexpected encoding {other}"),
            }
        }
        // Groups are recorded per encoding; the plan merges them in file order.
        let groups = [
            OffsetGroup::new(OffsetEncoding::Utf32Be, OffsetRanges::Text(utf32)).unwrap(),
            OffsetGroup::new(OffsetEncoding::Utf8, OffsetRanges::Text(utf8)).unwrap(),
        ];
        let recorded = TextPlan::from_recorded(
            &source[..scanned.region_end],
            &groups,
            "/PFX",
            "/new",
            &Subdir::Linux64,
        )
        .unwrap();
        assert_eq!(recorded, scanned);
    }

    #[test]
    fn binary_wide_cstrings() {
        // Wide c-strings are terminated by a zero code unit and padded with
        // zeros up to it, keeping the file length.
        for encoding in WIDE {
            let unit = encoding.code_unit_size();
            let mut source = b"head".to_vec();
            source.resize(16, 0);
            source.extend(encoding.encode("/PFX/lib:/PFX/bin"));
            source.extend(vec![0; unit]);
            source.extend_from_slice(b"tail");

            let mut expected = b"head".to_vec();
            expected.resize(16, 0);
            expected.extend(encoding.encode("/np/lib:/np/bin"));
            // Two replacements, each one code unit shorter.
            expected.extend(vec![0; 2 * unit]);
            expected.extend(vec![0; unit]);
            expected.extend_from_slice(b"tail");

            let cstrings = plan_binary_replacement(&source, "/PFX");
            assert_eq!(cstrings.len(), 1, "{encoding}");
            assert_eq!(cstrings[0].encoding, encoding);
            assert_eq!(cstrings[0].offsets.len(), 2);

            let full = binary_read(&source, "/PFX", "/np", 0, source.len());
            assert_eq!(full, expected, "{encoding}");
            for start in 0..source.len() {
                for len in [1usize, 3, 9] {
                    let end = (start + len).min(source.len());
                    let out = binary_read(&source, "/PFX", "/np", start, end);
                    assert_eq!(out, &expected[start..end], "{encoding} [{start}, {end})");
                }
            }
        }
    }

    #[test]
    fn binary_recorded_groups_merge_encodings() {
        // A UTF-8 c-string followed by a UTF-16-LE one, recorded as two groups.
        let mut source = b"/PFX/a\0\0".to_vec();
        // The UTF-16 string is code-unit aligned, as compilers emit it.
        let wide_start = source.len();
        source.extend(OffsetEncoding::Utf16Le.encode("/PFX/b"));
        let wide_nul = source.len();
        source.extend([0, 0]);

        let groups = [
            OffsetGroup::new(
                OffsetEncoding::Utf16Le,
                OffsetRanges::Binary(vec![vec![wide_start, wide_nul]]),
            )
            .unwrap(),
            OffsetGroup::new(OffsetEncoding::Utf8, OffsetRanges::Binary(vec![vec![0, 6]])).unwrap(),
        ];
        let recorded = cstrings_from_recorded(&groups);
        assert_eq!(recorded, plan_binary_replacement(&source, "/PFX"));

        let prefixes = EncodedPrefixes::new("/PFX", "/n");
        let out = binary_ranged_read(&source, &prefixes, &recorded, 0, source.len());
        let mut expected = b"/n/a\0\0\0\0".to_vec();
        expected.extend(OffsetEncoding::Utf16Le.encode("/n/b"));
        expected.extend([0; 4 + 2]);
        assert_eq!(out, expected);
    }

    #[test]
    fn binary_growing_target_is_served_unchanged() {
        // The installer refuses a target longer than the placeholder; the
        // mount must not panic and leaves the c-string as-is.
        let source = b"x/PFX/lib\0tail";
        let out = binary_read(source, "/PFX", "/much/longer", 0, source.len());
        assert_eq!(out, source);
    }
}
