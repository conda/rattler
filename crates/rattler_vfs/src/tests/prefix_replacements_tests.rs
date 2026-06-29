#[cfg(test)]
mod tests {
    use crate::metadata::CustomPrefixPlaceholder;
    use crate::prefix_replacement::{
        binary_prefix_replacement, find_unfinished_replacements, text_prefix_replacement,
    };
    use memmap2::{Mmap, MmapOptions};
    use rattler_conda_types::package::{FileMode, PrefixPlaceholder};
    use std::path::Path;

    fn mmap_bytes(bytes: &[u8]) -> Mmap {
        // map_anon requires len > 0; empty files are handled by returning early in the functions.
        if bytes.is_empty() {
            return MmapOptions::new()
                .len(1)
                .map_anon()
                .unwrap()
                .make_read_only()
                .unwrap();
        }
        let mut mmap = MmapOptions::new().len(bytes.len()).map_anon().unwrap();
        mmap[..].copy_from_slice(bytes);
        mmap.make_read_only().unwrap()
    }

    fn make_placeholder(
        file_mode: FileMode,
        placeholder: &str,
        bytes: &[u8],
    ) -> CustomPrefixPlaceholder {
        CustomPrefixPlaceholder::from_placeholder(
            PrefixPlaceholder {
                file_mode,
                placeholder: placeholder.to_string(),
            },
            bytes,
        )
    }

    fn do_text_test(
        placeholder: &str,
        prefix: &str,
        before: &[u8],
        expected: &[u8],
        start: usize,
        end: usize,
    ) {
        let mmap = mmap_bytes(before);
        let ph = make_placeholder(FileMode::Text, placeholder, before);
        let size = end - start;
        let result = text_prefix_replacement(&ph, start, end, size, &mmap, Path::new(prefix));
        assert_eq!(
            result, expected,
            "text replacement failed for {before:?} expected {expected:?} [{start}..{end}]"
        );
    }

    fn do_binary_test(
        placeholder: &str,
        prefix: &str,
        before: &[u8],
        expected: &[u8],
        start: usize,
        end: usize,
    ) {
        let mmap = mmap_bytes(before);
        let ph = make_placeholder(FileMode::Binary, placeholder, before);
        let size = end - start;
        let result = binary_prefix_replacement(&ph, start, end, size, &mmap, Path::new(prefix));
        assert_eq!(
            result, expected,
            "binary replacement failed for {before:?} expected {expected:?} [{start}..{end}]"
        );
    }

    // --- find_unfinished_replacements ---

    #[test]
    fn test_find_one_unfinished_replacements() {
        let file_before = b"01ABCD2\x0034ABCD5";
        let offsets = vec![2, 10];
        assert_eq!(find_unfinished_replacements(file_before, &offsets), 1);
    }

    #[test]
    fn test_find_two_unfinished_replacements_no_null_byte() {
        let file_before = b"01ABCD234ABCD5";
        let offsets = vec![2, 9];
        assert_eq!(find_unfinished_replacements(file_before, &offsets), 2);
    }

    // --- text replacement ---

    #[test]
    fn test_text_prefix_replacement_full_file() {
        do_text_test(
            "ABCD",
            "XY",
            b"01ABCD23456ABCD7890",
            b"01XY23456XY7890",
            0,
            b"01ABCD23456ABCD7890".len(),
        );
    }

    #[test]
    fn test_text_prefix_replacement_partial_range() {
        let before = b"ABCD0ABCD5ABCD0ABCD5ABCD";
        // replaced = "XY0XY5XY0XY5XY"; [2..9] = "0XY5XY0"
        do_text_test("ABCD", "XY", before, b"0XY5XY0", 2, 9);
    }

    #[test]
    fn test_text_prefix_replacement_start_after_prefix() {
        let before = b"ABCD01234ABCD56789";
        // replaced = "XY01234XY56789"; [5..] = "34XY56789"
        do_text_test("ABCD", "XY", before, b"34XY56789", 5, before.len());
    }

    #[test]
    fn test_text_prefix_replacement_start_between_placeholders() {
        let before = b"ABCD0123ABCD5678ABCD";
        // replaced = "XY0123XY5678XY"; [5..] = "3XY5678XY"
        do_text_test("ABCD", "XY", before, b"3XY5678XY", 5, before.len());
    }

    #[test]
    fn test_text_prefix_replacement_start_at_placeholder() {
        do_text_test(
            "ABCD",
            "XY",
            b"01234ABCD6789ABCD",
            b"XY6789XY",
            5,
            b"01234ABCD6789ABCD".len(),
        );
    }

    #[test]
    fn test_text_prefix_replacement_no_placeholders() {
        do_text_test(
            "ABCD",
            "XY",
            b"0123456789",
            b"0123456789",
            0,
            b"0123456789".len(),
        );
    }

    #[test]
    fn test_text_prefix_replacement_only_placeholder() {
        do_text_test("ABCD", "XY", b"ABCD", b"XY", 0, b"ABCD".len());
    }

    #[test]
    fn test_text_prefix_replacement_start_with_placeholder() {
        do_text_test(
            "ABCD",
            "XY",
            b"ABCD01234",
            b"XY01234",
            0,
            b"ABCD01234".len(),
        );
    }

    #[test]
    fn test_text_prefix_replacement_end_with_placeholder() {
        do_text_test(
            "ABCD",
            "XY",
            b"01234ABCD",
            b"01234XY",
            0,
            b"01234ABCD".len(),
        );
    }

    #[test]
    fn test_text_prefix_replacement_consecutive_placeholders() {
        do_text_test("ABCD", "XY", b"ABCDABCD", b"XYXY", 0, b"ABCDABCD".len());
    }

    #[test]
    fn test_text_prefix_replacement_same_length() {
        do_text_test(
            "ABCD",
            "WXYZ",
            b"01ABCD6789012ABCD7890",
            b"01WXYZ6789012WXYZ7890",
            0,
            b"01ABCD6789012ABCD7890".len(),
        );
    }

    #[test]
    fn test_text_prefix_replacement_empty_file() {
        // Empty input: start=0, end=0 → empty output
        let ph = make_placeholder(FileMode::Text, "ABCD", b"");
        let mmap = mmap_bytes(b"");
        let result = text_prefix_replacement(&ph, 0, 0, 0, &mmap, Path::new("XY"));
        assert_eq!(result, b"");
    }

    #[test]
    fn test_text_prefix_replacement_single_char_placeholder() {
        do_text_test("X", "A", b"0X2X4X6X8", b"0A2A4A6A8", 0, b"0X2X4X6X8".len());
    }

    #[test]
    fn test_text_prefix_replacement_many_placeholders() {
        let mut before = Vec::new();
        let mut expected = Vec::new();
        for i in 0..10 {
            before.extend_from_slice(format!("{i:02}ABCD").as_bytes());
            expected.extend_from_slice(format!("{i:02}XY").as_bytes());
        }
        do_text_test("ABCD", "XY", &before, &expected, 0, before.len());
    }

    #[test]
    fn test_text_prefix_replacement_longer_prefix() {
        do_text_test(
            "ABCDEFGH",
            "XYZ",
            b"01ABCDEFGH234ABCDEFGH567",
            b"01XYZ234XYZ567",
            0,
            b"01ABCDEFGH234ABCDEFGH567".len(),
        );
    }

    #[test]
    fn test_text_prefix_replacement_three_char_to_one() {
        do_text_test(
            "ABC",
            "X",
            b"00ABC11ABC22ABC33",
            b"00X11X22X33",
            0,
            b"00ABC11ABC22ABC33".len(),
        );
    }

    #[test]
    fn test_text_prefix_replacement_with_special_chars() {
        let before = b"{\n  \"path\": \"ABCD/file\",\n  \"root\": \"ABCD\"\n}";
        do_text_test(
            "ABCD",
            "XY",
            before,
            b"{\n  \"path\": \"XY/file\",\n  \"root\": \"XY\"\n}",
            0,
            before.len(),
        );
    }

    #[test]
    fn test_text_prefix_replacement_placeholder_at_boundary() {
        do_text_test("ABCD", "XY", b"01234567ABCD", b"01234567XY", 0, 12);
    }

    // --- binary replacement ---

    #[test]
    fn test_binary_prefix_replacement_full_file() {
        do_binary_test(
            "ABCD",
            "XY",
            b"01ABCD23\x00456ABCD78\x0090",
            b"01XY23\x00\x00\x00456XY78\x00\x00\x0090",
            0,
            b"01ABCD23\x00456ABCD78\x0090".len(),
        );
    }

    #[test]
    fn test_binary_prefix_replacement_no_placeholders() {
        do_binary_test(
            "ABCD",
            "XY",
            b"0123456789",
            b"0123456789",
            0,
            b"0123456789".len(),
        );
    }

    #[test]
    fn test_binary_prefix_replacement_only_placeholder() {
        do_binary_test("ABCD", "XY", b"ABCD", b"XY\x00\x00", 0, b"ABCD".len());
    }

    #[test]
    fn test_binary_prefix_replacement_start_with_placeholder() {
        do_binary_test(
            "ABCD",
            "XY",
            b"ABCD\x0001234",
            b"XY\x00\x00\x0001234",
            0,
            b"ABCD\x0001234".len(),
        );
    }

    #[test]
    fn test_binary_prefix_replacement_end_with_placeholder() {
        do_binary_test(
            "ABCD",
            "XY",
            b"01234ABCD",
            b"01234XY\x00\x00",
            0,
            b"01234ABCD".len(),
        );
    }

    #[test]
    fn test_binary_prefix_replacement_consecutive_placeholders() {
        do_binary_test(
            "ABCD",
            "XY",
            b"ABCDABCD",
            b"XYXY\x00\x00\x00\x00",
            0,
            b"ABCDABCD".len(),
        );
    }

    #[test]
    fn test_binary_prefix_replacement_same_length() {
        do_binary_test(
            "ABCD",
            "WXYZ",
            b"01ABCD6789012ABCD7890",
            b"01WXYZ6789012WXYZ7890",
            0,
            b"01ABCD6789012ABCD7890".len(),
        );
    }

    #[test]
    fn test_binary_prefix_replacement_empty_file() {
        let ph = make_placeholder(FileMode::Binary, "ABCD", b"");
        let mmap = mmap_bytes(b"");
        let result = binary_prefix_replacement(&ph, 0, 0, 0, &mmap, Path::new("XY"));
        assert_eq!(result, b"");
    }

    #[test]
    fn test_binary_prefix_replacement_single_char_placeholder() {
        do_binary_test("X", "A", b"0X2X4X6X8", b"0A2A4A6A8", 0, b"0X2X4X6X8".len());
    }

    #[test]
    fn test_binary_prefix_replacement_many_placeholders() {
        let mut before = Vec::new();
        let mut expected = Vec::new();
        for i in 0..10 {
            before.extend_from_slice(format!("{i:02}ABCD\x00").as_bytes());
            expected.extend_from_slice(format!("{i:02}XY\x00\x00\x00").as_bytes());
        }
        do_binary_test("ABCD", "XY", &before, &expected, 0, before.len());
    }

    #[test]
    fn test_binary_prefix_replacement_longer_prefix() {
        do_binary_test(
            "ABCDEFGH",
            "XYZ",
            b"01ABCDEFGH234ABCDEFGH567",
            b"01XYZ234XYZ567\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00",
            0,
            b"01ABCDEFGH234ABCDEFGH567".len(),
        );
    }

    #[test]
    fn test_binary_prefix_replacement_three_char_to_one() {
        do_binary_test(
            "ABC",
            "X",
            b"00ABC11ABC22ABC33",
            b"00X11X22X33\x00\x00\x00\x00\x00\x00",
            0,
            b"00ABC11ABC22ABC33".len(),
        );
    }

    #[test]
    fn test_binary_prefix_replacement_placeholder_at_boundary() {
        do_binary_test("ABCD", "XY", b"01234567ABCD", b"01234567XY\x00\x00", 0, 12);
    }

    #[test]
    fn test_binary_replacement_full_file_multiple_placeholders() {
        let before = b"\x00\x00ABCDZ\x00\x00\x00ABCDEFABCDEF\x00\x00\x00ABCDMNOPQRSABCDMNOPQRSABCDMNOPQRS\x00\x00";
        let expected = b"\x00\x00XYZ\x00\x00\x00\x00\x00XYEFXYEF\x00\x00\x00\x00\x00\x00\x00XYMNOPQRSXYMNOPQRSXYMNOPQRS\x00\x00\x00\x00\x00\x00\x00\x00";
        do_binary_test("ABCD", "XY", before, expected, 0, before.len());
    }
}
