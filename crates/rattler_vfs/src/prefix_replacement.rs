//! Ranged prefix replacement for FUSE/NFS reads.
//!
//! Serves a byte range `[start, end)` from a source file with prefix
//! placeholder replacements applied on the fly, without materializing the
//! entire transformed file in memory.

use memchr::memmem;

/// Read a range from a text file with prefix replacements applied.
///
/// Text-mode replacement changes `old_prefix` → `new_prefix` at each offset.
/// Because the replacement can change length, output byte positions shift
/// relative to source positions. The `offsets` array gives the source-file
/// positions of each placeholder occurrence.
///
/// Returns the bytes in the output range `[start, end)`.
pub fn text_ranged_read(
    source: &[u8],
    old_prefix: &[u8],
    new_prefix: &[u8],
    offsets: &[usize],
    start: usize,
    end: usize,
) -> Vec<u8> {
    let delta = new_prefix.len() as isize - old_prefix.len() as isize;
    let transformed_len = (source.len() as isize + delta * offsets.len() as isize).max(0) as usize;

    let actual_end = end.min(transformed_len);
    let actual_start = start.min(transformed_len);
    if actual_start >= actual_end {
        return vec![];
    }

    let capacity = actual_end - actual_start;
    let mut buffer = Vec::with_capacity(capacity);

    // Walk through the source, tracking the current position in both
    // source space and output (transformed) space.
    let mut src_pos = 0usize;
    let mut out_pos = 0usize;
    let mut offset_idx = 0usize;

    while src_pos < source.len() && buffer.len() < capacity {
        if offset_idx < offsets.len() && src_pos == offsets[offset_idx] {
            // At a replacement site: emit new_prefix bytes
            for &b in new_prefix {
                if out_pos >= actual_start && out_pos < actual_end {
                    buffer.push(b);
                }
                out_pos += 1;
                if buffer.len() >= capacity {
                    return buffer;
                }
            }
            // Skip the old prefix in the source
            src_pos += old_prefix.len();
            offset_idx += 1;
        } else {
            // Regular byte: copy through
            if out_pos >= actual_start && out_pos < actual_end {
                buffer.push(source[src_pos]);
            }
            out_pos += 1;
            src_pos += 1;
        }
    }

    buffer
}

/// Read a range from a binary file with prefix replacements applied.
///
/// Binary-mode replacement swaps `old_prefix` → `new_prefix` and pads with
/// null bytes to maintain the same total length. Offsets are grouped by
/// c-string: each inner slice lists prefix start positions followed by the
/// NUL terminator position.
///
/// Output length always equals source length.
///
/// Returns the bytes in the output range `[start, end)`.
pub fn binary_ranged_read(
    source: &[u8],
    old_prefix: &[u8],
    new_prefix: &[u8],
    groups: &[Vec<usize>],
    start: usize,
    end: usize,
) -> Vec<u8> {
    assert!(
        new_prefix.len() <= old_prefix.len(),
        "new prefix cannot be longer than old prefix in binary mode"
    );

    let actual_end = end.min(source.len());
    let actual_start = start.min(source.len());
    if actual_start >= actual_end {
        return vec![];
    }

    let length_change = old_prefix.len() - new_prefix.len();
    let capacity = actual_end - actual_start;
    let mut buffer = Vec::with_capacity(capacity);

    // Build a virtual stream of the fully-replaced file, emitting only
    // the bytes that fall within [actual_start, actual_end).
    let mut out_pos: usize = 0; // current position in the output stream
    let mut src_pos: usize = 0; // current position in the source

    for group in groups {
        let (prefix_offsets, nul_slice) = group.split_at(group.len() - 1);
        let nul_pos = nul_slice[0];

        for &offset in prefix_offsets {
            // Emit source bytes from src_pos to this prefix
            emit_range(
                source,
                src_pos,
                offset,
                &mut out_pos,
                actual_start,
                actual_end,
                &mut buffer,
            );
            src_pos = offset;

            // Emit new prefix
            emit_bytes(
                new_prefix,
                &mut out_pos,
                actual_start,
                actual_end,
                &mut buffer,
            );
            src_pos += old_prefix.len();
        }

        // Emit source bytes from last prefix end to NUL position
        emit_range(
            source,
            src_pos,
            nul_pos,
            &mut out_pos,
            actual_start,
            actual_end,
            &mut buffer,
        );
        src_pos = nul_pos;

        // Emit padding zeros
        let padding = prefix_offsets.len() * length_change;
        emit_zeros(padding, &mut out_pos, actual_start, actual_end, &mut buffer);

        if buffer.len() >= capacity {
            break;
        }
    }

    // Emit remaining source bytes after the last group
    if src_pos < source.len() && buffer.len() < capacity {
        emit_range(
            source,
            src_pos,
            source.len(),
            &mut out_pos,
            actual_start,
            actual_end,
            &mut buffer,
        );
    }

    buffer
}

/// Emit bytes from `source[src_start..src_end]` into buffer, but only those
/// that fall within the output window `[win_start, win_end)`.
#[inline]
fn emit_range(
    source: &[u8],
    src_start: usize,
    src_end: usize,
    out_pos: &mut usize,
    win_start: usize,
    win_end: usize,
    buffer: &mut Vec<u8>,
) {
    for &b in &source[src_start..src_end] {
        if *out_pos >= win_start && *out_pos < win_end {
            buffer.push(b);
        }
        *out_pos += 1;
        if *out_pos >= win_end {
            return;
        }
    }
}

/// Emit a slice of bytes into buffer within the output window.
#[inline]
fn emit_bytes(
    data: &[u8],
    out_pos: &mut usize,
    win_start: usize,
    win_end: usize,
    buffer: &mut Vec<u8>,
) {
    for &b in data {
        if *out_pos >= win_start && *out_pos < win_end {
            buffer.push(b);
        }
        *out_pos += 1;
        if *out_pos >= win_end {
            return;
        }
    }
}

/// Emit `count` zero bytes into buffer within the output window.
#[inline]
fn emit_zeros(
    count: usize,
    out_pos: &mut usize,
    win_start: usize,
    win_end: usize,
    buffer: &mut Vec<u8>,
) {
    for _ in 0..count {
        if *out_pos >= win_start && *out_pos < win_end {
            buffer.push(0);
        }
        *out_pos += 1;
        if *out_pos >= win_end {
            return;
        }
    }
}

/// Compute text-mode replacement offsets by scanning the source for the placeholder.
/// Used when paths.json doesn't provide offsets (legacy v1 format).
pub fn collect_offsets(source: &[u8], placeholder: &[u8]) -> Vec<usize> {
    memmem::find_iter(source, placeholder).collect()
}

/// Compute binary-mode replacement offsets grouped by c-string.
/// Each inner Vec lists the prefix start positions followed by the NUL
/// terminator position: `[prefix1, prefix2, nul_pos]`.
/// Used when paths.json doesn't provide offsets (legacy v1 format).
pub fn collect_binary_offsets(source: &[u8], placeholder: &[u8]) -> Vec<Vec<usize>> {
    let flat: Vec<usize> = memmem::find_iter(source, placeholder).collect();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut current_group: Vec<usize> = Vec::new();

    for offset in flat {
        // Check if there's a NUL between the previous offset's end and this one
        if let Some(&prev) = current_group.last() {
            let search_start = prev + placeholder.len();
            if let Some(nul_pos) = memchr::memchr(b'\0', &source[search_start..offset]) {
                // NUL found — close previous group
                current_group.push(search_start + nul_pos);
                groups.push(std::mem::take(&mut current_group));
            }
        }
        current_group.push(offset);
    }

    // Close the last group — find the NUL after the last prefix
    if !current_group.is_empty() {
        let last_end = current_group.last().unwrap() + placeholder.len();
        let nul_pos =
            memchr::memchr(b'\0', &source[last_end..]).map_or(source.len(), |p| last_end + p);
        current_group.push(nul_pos);
        groups.push(current_group);
    }

    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Text mode tests ──────────────────────────────────────────────

    fn text_test(
        placeholder: &[u8],
        prefix: &[u8],
        source: &[u8],
        expected: &[u8],
        start: usize,
        end: usize,
    ) {
        let offsets = collect_offsets(source, placeholder);
        let result = text_ranged_read(source, placeholder, prefix, &offsets, start, end);
        assert_eq!(
            result, expected,
            "text replacement [{start}..{end}] of {source:?}: expected {expected:?}, got {result:?}"
        );
    }

    fn binary_test(
        placeholder: &[u8],
        prefix: &[u8],
        source: &[u8],
        expected: &[u8],
        start: usize,
        end: usize,
    ) {
        let groups = collect_binary_offsets(source, placeholder);
        let result = binary_ranged_read(source, placeholder, prefix, &groups, start, end);
        assert_eq!(
            result, expected,
            "binary replacement [{start}..{end}] of {source:?}: expected {expected:?}, got {result:?}"
        );
    }

    // Full-file text replacements

    #[test]
    fn text_full_file() {
        text_test(
            b"ABCD",
            b"XY",
            b"01ABCD23456ABCD7890",
            b"01XY23456XY7890",
            0,
            19,
        );
    }

    #[test]
    fn text_only_placeholder() {
        text_test(b"ABCD", b"XY", b"ABCD", b"XY", 0, 4);
    }

    #[test]
    fn text_consecutive_placeholders() {
        text_test(b"ABCD", b"XY", b"ABCDABCD", b"XYXY", 0, 8);
    }

    #[test]
    fn text_no_placeholders() {
        text_test(b"ABCD", b"XY", b"0123456789", b"0123456789", 0, 10);
    }

    #[test]
    fn text_same_length() {
        text_test(
            b"ABCD",
            b"WXYZ",
            b"01ABCD6789012ABCD7890",
            b"01WXYZ6789012WXYZ7890",
            0,
            21,
        );
    }

    #[test]
    fn text_empty_file() {
        text_test(b"ABCD", b"XY", b"", b"", 0, 0);
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
        text_test(b"ABCD", b"XY", &source, &expected, 0, source.len());
    }

    // Partial-range text replacements

    #[test]
    fn text_partial_range() {
        text_test(
            b"ABCD",
            b"XY",
            b"ABCD0ABCD5ABCD0ABCD5ABCD",
            b"0XY5XY0",
            2,
            9,
        );
    }

    #[test]
    fn text_start_after_prefix() {
        // Output: "XY01234XY56789" (14 chars) — start at index 5 = "34XY56789"
        text_test(b"ABCD", b"XY", b"ABCD01234ABCD56789", b"34XY56789", 5, 18);
    }

    #[test]
    fn text_start_between_placeholders() {
        // Output: "XY0123XY5678XY" — start at index 5 = "3XY5678XY"
        text_test(b"ABCD", b"XY", b"ABCD0123ABCD5678ABCD", b"3XY5678XY", 5, 20);
    }

    #[test]
    fn text_start_at_placeholder() {
        // Output: "01234XY6789XY" — start at index 5 = "XY6789XY"
        text_test(b"ABCD", b"XY", b"01234ABCD6789ABCD", b"XY6789XY", 5, 17);
    }

    #[test]
    fn text_longer_placeholder() {
        text_test(
            b"ABCDEFGH",
            b"XYZ",
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
            b"ABCD",
            b"XY",
            b"01ABCD23\x00456ABCD78\x0090",
            b"01XY23\x00\x00\x00456XY78\x00\x00\x0090",
            0,
            21,
        );
    }

    #[test]
    fn binary_only_placeholder() {
        binary_test(b"ABCD", b"XY", b"ABCD", b"XY\x00\x00", 0, 4);
    }

    #[test]
    fn binary_no_placeholders() {
        binary_test(b"ABCD", b"XY", b"0123456789", b"0123456789", 0, 10);
    }

    #[test]
    fn binary_same_length() {
        binary_test(
            b"ABCD",
            b"WXYZ",
            b"01ABCD6789012ABCD7890",
            b"01WXYZ6789012WXYZ7890",
            0,
            21,
        );
    }

    #[test]
    fn binary_consecutive_placeholders() {
        binary_test(b"ABCD", b"XY", b"ABCDABCD", b"XYXY\x00\x00\x00\x00", 0, 8);
    }

    #[test]
    fn binary_multiple_placeholders() {
        let source = b"\x00\x00ABCDZ\x00\x00\x00ABCDEFABCDEF\x00\x00\x00ABCDMNOPQRSABCDMNOPQRSABCDMNOPQRS\x00\x00";
        let expected = b"\x00\x00XYZ\x00\x00\x00\x00\x00XYEFXYEF\x00\x00\x00\x00\x00\x00\x00XYMNOPQRSXYMNOPQRSXYMNOPQRS\x00\x00\x00\x00\x00\x00\x00\x00";
        binary_test(b"ABCD", b"XY", source, expected, 0, source.len());
    }

    #[test]
    fn binary_empty_file() {
        binary_test(b"ABCD", b"XY", b"", b"", 0, 0);
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
        binary_test(b"ABCD", b"XY", &source, &expected, 0, source.len());
    }

    // Partial-range binary replacements

    #[test]
    fn binary_partial_range() {
        binary_test(
            b"ABCD",
            b"XY",
            b"ABCD\x000ABCD\x005ABCD\x000ABCD\x005ABCD\x00",
            b"0XY\x00\x00",
            5,
            10,
        );
    }

    #[test]
    fn binary_start_after_prefix() {
        binary_test(
            b"ABCD",
            b"XY",
            b"ABCD01234ABCD\x0056789",
            b"34XY\x00\x00\x00\x00\x0056789",
            5,
            19,
        );
    }

    #[test]
    fn binary_start_between_placeholders() {
        binary_test(
            b"ABCD",
            b"XY",
            b"ABCD012\x003ABCD5678ABCD",
            b"012\x00\x00\x003XY5678XY\x00\x00\x00\x00",
            2,
            21,
        );
    }

    #[test]
    fn binary_start_at_placeholder() {
        binary_test(
            b"ABCD",
            b"XY",
            b"01234ABCD\x006789ABCD\x00",
            b"XY\x00\x00\x006789XY\x00\x00\x00",
            5,
            19,
        );
    }

    #[test]
    fn binary_longer_placeholder() {
        binary_test(
            b"ABCDEFGH",
            b"XYZ",
            b"01ABCDEFGH234ABCDEFGH567",
            b"01XYZ234XYZ567\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00",
            0,
            24,
        );
    }

    // ── Offset collection ────────────────────────────────────────────

    #[test]
    fn collect_offsets_basic() {
        assert_eq!(collect_offsets(b"01ABCD56ABCD", b"ABCD"), vec![2, 8]);
    }

    #[test]
    fn collect_offsets_none() {
        assert_eq!(collect_offsets(b"0123456789", b"ABCD"), Vec::<usize>::new());
    }

    #[test]
    fn collect_offsets_consecutive() {
        assert_eq!(collect_offsets(b"ABCDABCD", b"ABCD"), vec![0, 4]);
    }

    #[test]
    fn collect_binary_offsets_single() {
        // One prefix in one c-string
        assert_eq!(
            collect_binary_offsets(b"hello/PFX/bin\x00tail", b"/PFX"),
            vec![vec![5, 13]]
        );
    }

    #[test]
    fn collect_binary_offsets_multi_in_one_cstring() {
        // Two prefixes sharing one c-string (PATH-style)
        assert_eq!(
            collect_binary_offsets(b"PATH=/PFX/a:/PFX/b\x00tail", b"/PFX"),
            vec![vec![5, 12, 18]]
        );
    }

    #[test]
    fn collect_binary_offsets_separate_cstrings() {
        // Two prefixes in separate c-strings
        assert_eq!(
            collect_binary_offsets(b"/PFX/a\x00/PFX/b\x00", b"/PFX"),
            vec![vec![0, 6], vec![7, 13]]
        );
    }
}
