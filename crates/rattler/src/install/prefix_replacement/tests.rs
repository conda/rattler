use fs_err as fs;
use rattler_conda_types::Platform;
use rattler_conda_types::package::{OffsetEncoding, OffsetGroup, OffsetRanges};
use rstest::rstest;
use std::io::Cursor;

/// Builds the UTF-8 offset group a producer conformant with the draft CEP would emit.
fn utf8_group(ranges: OffsetRanges) -> OffsetGroup {
    OffsetGroup {
        encoding: OffsetEncoding::Utf8,
        ranges,
        unknown_members: vec![],
    }
}

/// Builds the offset group a producer conformant with the draft CEP emits for a text file whose
/// occurrences are all UTF-8. An empty list means the file has nothing to splice outside its
/// shebang.
fn utf8_text_groups(offsets: &[usize]) -> Vec<OffsetGroup> {
    if offsets.is_empty() {
        return Vec::new();
    }
    vec![utf8_group(OffsetRanges::Text(offsets.to_vec()))]
}

/// Builds the offset group a producer conformant with the draft CEP emits for a binary file
/// whose occurrences are all UTF-8, grouped by c-string.
fn utf8_binary_groups(cstrings: &[Vec<usize>]) -> Vec<OffsetGroup> {
    vec![utf8_group(OffsetRanges::Binary(cstrings.to_vec()))]
}

/// Encodes `text` with `encoding`, for building wide-string test fixtures.
fn encode(encoding: &OffsetEncoding, text: &str) -> Vec<u8> {
    encoding.encode(text).expect("a defined encoding")
}

#[rstest]
#[case("Hello, cruel world!", "cruel", "fabulous", "Hello, fabulous world!")]
#[case(
    "prefix_placeholder",
    "prefix_placeholder",
    "target_prefix",
    "target_prefix"
)]
pub fn test_copy_and_replace_textual_placeholder(
    #[case] input: &str,
    #[case] prefix_placeholder: &str,
    #[case] target_prefix: &str,
    #[case] expected_output: &str,
) {
    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder(
        input.as_bytes(),
        &mut output,
        prefix_placeholder,
        target_prefix,
        &Platform::Linux64,
    )
    .unwrap();
    assert_eq!(
        &String::from_utf8_lossy(&output.into_inner()),
        expected_output
    );
}

#[rstest]
#[case(
    b"12345Hello, fabulous world!\x006789",
    "fabulous",
    "cruel",
    b"12345Hello, cruel world!\x00\x00\x00\x006789"
)]
pub fn test_copy_and_replace_binary_placeholder(
    #[case] input: &[u8],
    #[case] prefix_placeholder: &str,
    #[case] target_prefix: &str,
    #[case] expected_output: &[u8],
) {
    assert_eq!(
        expected_output.len(),
        input.len(),
        "input and expected output must have the same length"
    );
    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_cstring_placeholder(
        input,
        &mut output,
        prefix_placeholder,
        target_prefix,
    )
    .unwrap();
    assert_eq!(&output.into_inner(), expected_output);
}

#[rstest]
#[case(b"short\x00", "short", "verylong")]
#[case(b"short1234\x00", "short", "verylong")]
pub fn test_shorter_binary_placeholder(
    #[case] input: &[u8],
    #[case] prefix_placeholder: &str,
    #[case] target_prefix: &str,
) {
    assert!(target_prefix.len() > prefix_placeholder.len());

    let mut output = Cursor::new(Vec::new());
    let result = super::copy_and_replace_cstring_placeholder(
        input,
        &mut output,
        prefix_placeholder,
        target_prefix,
    );
    assert!(result.is_err());
}

#[test]
fn replace_binary_path_var() {
    let input = b"beginrandomdataPATH=/placeholder/etc/share:/placeholder/bin/:\x00somemoretext";
    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_cstring_placeholder(input, &mut output, "/placeholder", "/target")
        .unwrap();
    let out = &output.into_inner();
    assert_eq!(out, b"beginrandomdataPATH=/target/etc/share:/target/bin/:\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00somemoretext");
    assert_eq!(out.len(), input.len());
}

#[test]
fn test_replace_shebang() {
    let shebang_with_spaces = "#!/path/placeholder/executable -o test -x".into();
    let replaced = super::text::replace_shebang(
        shebang_with_spaces,
        ("placeholder", "with space"),
        &Platform::Linux64,
    );
    assert_eq!(replaced, "#!/usr/bin/env executable -o test -x");
}

#[test]
fn test_replace_long_shebang() {
    let short_shebang = "#!/path/to/executable -x 123".into();
    let replaced = super::text::replace_shebang(short_shebang, ("", ""), &Platform::Linux64);
    assert_eq!(replaced, "#!/path/to/executable -x 123");

    let shebang = "#!/this/is/loooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooong/executable -o test -x";
    let replaced = super::text::replace_shebang(shebang.into(), ("", ""), &Platform::Linux64);
    assert_eq!(replaced, "#!/usr/bin/env executable -o test -x");

    let replaced = super::text::replace_shebang(shebang.into(), ("", ""), &Platform::Osx64);
    assert_eq!(replaced, shebang);

    let shebang_with_escapes = "#!/this/is/loooooooooooooooooooooooooooooooooooooooooooooooooooo\\ oooooo\\ oooooo\\ oooooooooooooooooooooooooooooooooooong/exe\\ cutable -o test -x";
    let replaced =
        super::text::replace_shebang(shebang_with_escapes.into(), ("", ""), &Platform::Linux64);
    assert_eq!(replaced, "#!/usr/bin/env exe\\ cutable -o test -x");

    let shebang = "#!    /this/is/looooooooooooooooooooooooooooooooooooooooooooo\\ \\ ooooooo\\ oooooo\\ oooooo\\ ooooooooooooooooo\\ ooooooooooooooooooong/exe\\ cutable -o \"te  st\" -x";
    let replaced = super::text::replace_shebang(shebang.into(), ("", ""), &Platform::Linux64);
    assert_eq!(replaced, "#!/usr/bin/env exe\\ cutable -o \"te  st\" -x");

    let shebang = "#!/usr/bin/env perl";
    let replaced = super::text::replace_shebang(
        shebang.into(),
        ("/placeholder", "/with space"),
        &Platform::Linux64,
    );
    assert_eq!(replaced, shebang);

    let shebang = "#!/placeholder/perl";
    let replaced = super::text::replace_shebang(
        shebang.into(),
        ("/placeholder", "/with space"),
        &Platform::Linux64,
    );
    assert_eq!(replaced, "#!/usr/bin/env perl");
}

#[test]
fn replace_python_shebang() {
    let short_shebang = "#!/path/to/python3.12".into();
    let replaced = super::text::replace_shebang(
        short_shebang,
        ("/path/to", "/new/prefix/with spaces/bin"),
        &Platform::Linux64,
    );
    insta::assert_snapshot!(replaced);

    let short_shebang = "#!/path/to/python3.12 -x 123".into();
    let replaced = super::text::replace_shebang(
        short_shebang,
        ("/path/to", "/new/prefix/with spaces/bin"),
        &Platform::Linux64,
    );
    insta::assert_snapshot!(replaced);
}

#[test]
fn test_replace_long_prefix_in_text_file() {
    let test_data_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data");
    let test_file = test_data_dir.join("shebang_test.txt");
    let prefix_placeholder = "/this/is/placeholder";
    let mut target_prefix = "/super/long/".to_string();
    for _ in 0..15 {
        target_prefix.push_str("verylongstring/");
    }
    let input = fs::read(test_file).unwrap();
    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder(
        &input,
        &mut output,
        prefix_placeholder,
        &target_prefix,
        &Platform::Linux64,
    )
    .unwrap();

    let output = output.into_inner();
    let replaced = String::from_utf8_lossy(&output);
    insta::assert_snapshot!(replaced);
}

#[test]
fn test_python_regex() {
    // Test the regex
    let test_strings = vec!["python", "python3", "python3.12", "python2.7"];

    for s in test_strings {
        assert!(super::text::PYTHON_REGEX.is_match(s));
    }

    let no_match_strings = vec![
        "python3.12.1",
        "python3.12.1.1",
        "foo",
        "foo3.2",
        "pythondoc",
    ];

    for s in no_match_strings {
        assert!(!super::text::PYTHON_REGEX.is_match(s));
    }
}

#[rstest]
#[case("Hello, cruel world!", [7].to_vec(), "cruel", "fabulous", "Hello, fabulous world!")]
#[case(
    "prefix_placeholder",
    [0].to_vec(),
    "prefix_placeholder",
    "target_prefix",
    "target_prefix"
)]
pub fn test_copy_and_replace_textual_placeholder_with_offsets(
    #[case] input: &str,
    #[case] offsets: Vec<usize>,
    #[case] prefix_placeholder: &str,
    #[case] target_prefix: &str,
    #[case] expected_output: &str,
) {
    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder_offsets(
        input.as_bytes(),
        &mut output,
        prefix_placeholder,
        target_prefix,
        &Platform::Linux64,
        &utf8_text_groups(&offsets),
        None,
    )
    .unwrap();
    assert_eq!(
        &String::from_utf8_lossy(&output.into_inner()),
        expected_output
    );
}

/// Records only the body occurrences in `offsets`, filtering out the ones inside the shebang
/// region, exactly what a producer conformant with the draft CEP emits.
fn conformant_text_offsets(input: &[u8], placeholder: &str) -> (Vec<usize>, Option<usize>) {
    let shebang_length = input.starts_with(b"#!").then(|| {
        input
            .iter()
            .position(|&c| c == b'\n')
            .map_or(input.len(), |i| i + 1)
    });
    let region_end = shebang_length.unwrap_or(0);
    let offsets = memchr::memmem::find_iter(input, placeholder.as_bytes())
        .filter(|&o| o >= region_end)
        .collect();
    (offsets, shebang_length)
}

/// A Unix target with a short target prefix. The occurrence inside the
/// shebang line (excluded from `offsets`) is rewritten by the shebang rules and, being short
/// enough, the patched line is kept; the body occurrence is spliced at its recorded offset.
#[test]
fn test_textual_offsets_shebang_kept_short_prefix() {
    let prefix_placeholder = "/this/is/placeholder";
    let target_prefix = "/opt/conda";
    let input =
        format!("#!{prefix_placeholder}/python\nimport sys  # see {prefix_placeholder}/lib\n")
            .into_bytes();

    let (offsets, shebang_length) = conformant_text_offsets(&input, prefix_placeholder);
    assert_eq!(offsets.len(), 1, "only the body occurrence is recorded");
    assert_eq!(shebang_length, Some(30));

    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder_offsets(
        &input,
        &mut output,
        prefix_placeholder,
        target_prefix,
        &Platform::Linux64,
        &utf8_text_groups(&offsets),
        shebang_length,
    )
    .unwrap();

    let expected = format!("#!{target_prefix}/python\nimport sys  # see {target_prefix}/lib\n");
    assert_eq!(String::from_utf8_lossy(&output.into_inner()), expected);
}

/// A target without shebang handling (Windows, e.g. a `noarch` package) with an
/// occurrence inside the shebang region. There is no shebang machinery, so the region MUST get
/// plain placeholder replacement even though its occurrence is not in `offsets`.
#[test]
fn test_textual_offsets_shebang_windows_plain_region() {
    let prefix_placeholder = "/this/is/placeholder";
    let target_prefix = "/opt/conda";
    let input =
        format!("#!{prefix_placeholder}/python\nimport sys  # see {prefix_placeholder}/lib\n")
            .into_bytes();

    let (offsets, shebang_length) = conformant_text_offsets(&input, prefix_placeholder);

    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder_offsets(
        &input,
        &mut output,
        prefix_placeholder,
        target_prefix,
        &Platform::Win64,
        &utf8_text_groups(&offsets),
        shebang_length,
    )
    .unwrap();

    // The shebang region's occurrence is replaced by the plain in-region path, not left behind.
    let expected = format!("#!{target_prefix}/python\nimport sys  # see {target_prefix}/lib\n");
    assert_eq!(String::from_utf8_lossy(&output.into_inner()), expected);
}

/// A shebang file with no trailing newline, where `shebang_length` equals the file size: the
/// whole file is the shebang line and there is no newline to copy through.
#[test]
fn test_textual_offsets_shebang_no_trailing_newline() {
    let prefix_placeholder = "/this/is/placeholder";
    let target_prefix = "/opt/conda";
    let input = format!("#!{prefix_placeholder}/python").into_bytes();

    let (offsets, shebang_length) = conformant_text_offsets(&input, prefix_placeholder);
    assert!(offsets.is_empty(), "the only occurrence is in the shebang");
    assert_eq!(shebang_length, Some(input.len()));

    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder_offsets(
        &input,
        &mut output,
        prefix_placeholder,
        target_prefix,
        &Platform::Linux64,
        &utf8_text_groups(&offsets),
        shebang_length,
    )
    .unwrap();

    assert_eq!(
        String::from_utf8_lossy(&output.into_inner()),
        format!("#!{target_prefix}/python")
    );
}

/// A file whose only occurrence is in the shebang line, so `offsets` is the empty list. The
/// shebang is short enough to keep.
#[test]
fn test_textual_offsets_only_shebang_occurrence_empty_offsets() {
    let prefix_placeholder = "/this/is/placeholder";
    let target_prefix = "/opt/conda";
    let input = format!("#!{prefix_placeholder}/python\nimport sys\n").into_bytes();

    let (offsets, shebang_length) = conformant_text_offsets(&input, prefix_placeholder);
    assert!(offsets.is_empty());

    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder_offsets(
        &input,
        &mut output,
        prefix_placeholder,
        target_prefix,
        &Platform::Linux64,
        &utf8_text_groups(&offsets),
        shebang_length,
    )
    .unwrap();

    assert_eq!(
        String::from_utf8_lossy(&output.into_inner()),
        format!("#!{target_prefix}/python\nimport sys\n")
    );
}

/// Multiple occurrences within one shebang line. All of them are in the region (so `offsets` is
/// empty) and the shebang rules replace them all.
#[test]
fn test_textual_offsets_multiple_occurrences_in_shebang() {
    let prefix_placeholder = "/this/is/placeholder";
    let target_prefix = "/opt/conda";
    let input = format!("#!{prefix_placeholder}/python -S {prefix_placeholder}/site\nprint(1)\n")
        .into_bytes();

    let (offsets, shebang_length) = conformant_text_offsets(&input, prefix_placeholder);
    assert!(
        offsets.is_empty(),
        "both occurrences are inside the shebang line"
    );

    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder_offsets(
        &input,
        &mut output,
        prefix_placeholder,
        target_prefix,
        &Platform::Linux64,
        &utf8_text_groups(&offsets),
        shebang_length,
    )
    .unwrap();

    assert_eq!(
        String::from_utf8_lossy(&output.into_inner()),
        format!("#!{target_prefix}/python -S {target_prefix}/site\nprint(1)\n")
    );
}

/// A shebang line longer than the kernel limit that contains no
/// occurrence of the placeholder. `shebang_length` is still present (the file starts with `#!`)
/// and the over-long line collapses to the `#!/usr/bin/env <program>` form regardless.
#[test]
fn test_textual_offsets_overlong_shebang_no_occurrence() {
    let prefix_placeholder = "/this/is/placeholder";
    let target_prefix = "/opt/conda";
    let long_shebang = "#!/this/is/loooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooong/executable -o test -x";
    assert!(long_shebang.len() > 127);
    let input = format!("{long_shebang}\nprint(1)\n").into_bytes();

    let (offsets, shebang_length) = conformant_text_offsets(&input, prefix_placeholder);
    assert!(offsets.is_empty(), "the placeholder does not occur at all");
    assert_eq!(shebang_length, Some(long_shebang.len() + 1));

    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder_offsets(
        &input,
        &mut output,
        prefix_placeholder,
        target_prefix,
        &Platform::Linux64,
        &utf8_text_groups(&offsets),
        shebang_length,
    )
    .unwrap();

    assert_eq!(
        String::from_utf8_lossy(&output.into_inner()),
        "#!/usr/bin/env executable -o test -x\nprint(1)\n"
    );
}

/// A producer conformant with the draft CEP never lists a shebang-region occurrence in
/// `offsets`. If a non-conformant producer does, the offset function reports inconsistent
/// metadata (writing nothing) so the installer falls back to search-based replacement, which
/// produces the same bytes.
#[test]
fn test_textual_offsets_shebang_occurrence_in_offsets_is_inconsistent() {
    let prefix_placeholder = "/this/is/placeholder";
    let target_prefix = "/opt/conda";
    let input =
        format!("#!{prefix_placeholder}/python\nimport sys  # see {prefix_placeholder}/lib\n")
            .into_bytes();
    let shebang_length = input.iter().position(|&c| c == b'\n').unwrap() + 1;

    // Non-conformant: lists BOTH occurrences, including the in-region one at offset 2.
    let non_conformant: Vec<usize> =
        memchr::memmem::find_iter(&input, prefix_placeholder.as_bytes()).collect();
    assert_eq!(non_conformant.len(), 2);

    let mut output = Cursor::new(Vec::new());
    let result = super::copy_and_replace_textual_placeholder_offsets(
        &input,
        &mut output,
        prefix_placeholder,
        target_prefix,
        &Platform::Linux64,
        &utf8_text_groups(&non_conformant),
        Some(shebang_length),
    );
    assert!(
        matches!(
            result,
            Err(super::OffsetReplaceError::InconsistentMetadata(_))
        ),
        "in-region offset must be rejected: {result:?}"
    );
    assert!(
        output.into_inner().is_empty(),
        "nothing is written, so the fallback starts from a clean destination"
    );

    // The search-based fallback produces the correct bytes.
    let mut fallback = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder(
        &input,
        &mut fallback,
        prefix_placeholder,
        target_prefix,
        &Platform::Linux64,
    )
    .unwrap();
    let expected = format!("#!{target_prefix}/python\nimport sys  # see {target_prefix}/lib\n");
    assert_eq!(String::from_utf8_lossy(&fallback.into_inner()), expected);
}

/// A file that starts with `#!` but carries no `shebang_length` is producer non-conformance and
/// must be reported as inconsistent metadata rather than mishandled.
#[test]
fn test_textual_offsets_shebang_length_absent_is_inconsistent() {
    let mut output = Cursor::new(Vec::new());
    let result = super::copy_and_replace_textual_placeholder_offsets(
        b"#!/this/is/placeholder/python\n",
        &mut output,
        "/this/is/placeholder",
        "/opt/conda",
        &Platform::Linux64,
        &[],
        None,
    );
    assert!(
        matches!(
            result,
            Err(super::OffsetReplaceError::InconsistentMetadata(_))
        ),
        "{result:?}"
    );
    assert!(output.into_inner().is_empty());
}

/// A `shebang_length` that disagrees with the first-newline position is inconsistent.
#[test]
fn test_textual_offsets_shebang_length_mismatch_is_inconsistent() {
    let mut output = Cursor::new(Vec::new());
    let result = super::copy_and_replace_textual_placeholder_offsets(
        b"#!/this/is/placeholder/python\nbody\n",
        &mut output,
        "/this/is/placeholder",
        "/opt/conda",
        &Platform::Linux64,
        &[],
        Some(20), // the correct value is 30
    );
    assert!(
        matches!(
            result,
            Err(super::OffsetReplaceError::InconsistentMetadata(_))
        ),
        "{result:?}"
    );
    assert!(output.into_inner().is_empty());
}

/// Regression for the search-based path: a `#!` file with no trailing newline must not panic.
/// Previously the missing newline yielded an empty "first line", tripping an assertion inside
/// `replace_shebang`.
#[test]
fn test_scan_path_shebang_without_newline_does_not_panic() {
    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder(
        b"#!/this/is/placeholder/python",
        &mut output,
        "/this/is/placeholder",
        "/opt/conda",
        &Platform::Linux64,
    )
    .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.into_inner()),
        "#!/opt/conda/python"
    );
}

/// A binary file whose final c-string is unterminated at end-of-file: the group's last value is
/// the file size and the length-preserving padding runs to EOF.
#[test]
fn test_binary_offsets_unterminated_final_cstring() {
    let input = b"AAAA/placeholder";
    let groups = vec![vec![4, input.len()]];

    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_cstring_placeholder_offsets(
        input,
        &mut output,
        "/placeholder",
        "/opt",
        &utf8_binary_groups(&groups),
    )
    .unwrap();

    let out = output.into_inner();
    assert_eq!(out, b"AAAA/opt\0\0\0\0\0\0\0\0");
    assert_eq!(out.len(), input.len(), "length must be preserved");
}

/// The dispatcher applies the UTF-8 group's ranges for a text file.
#[test]
fn test_offset_groups_text_utf8_group_applied() {
    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_placeholders_with_offsets(
        b"Hello, cruel world!",
        &mut output,
        "cruel",
        "fabulous",
        &Platform::Linux64,
        super::FileMode::Text,
        &[utf8_group(OffsetRanges::Text(vec![7]))],
        None,
    )
    .unwrap();
    assert_eq!(output.into_inner(), b"Hello, fabulous world!");
}

/// A binary file with occurrences under more than one encoding. Replacement covers every
/// encoding the draft CEP defines, so both the UTF-8 c-string and the UTF-16-LE wide string are
/// patched, and the offsets path reproduces what the search finds. The file length is preserved
/// either way.
#[test]
fn test_offset_groups_binary_multi_encoding() {
    let placeholder = "/pfx";
    let target = "/np";

    // A UTF-8 c-string with the placeholder at offset 1 (NUL at 9),
    // followed by a UTF-16-LE wide string with the placeholder at offset
    // 10 (two-byte NUL terminator starting at 28), followed by a tail.
    let wide = encode(&OffsetEncoding::Utf16Le, "/pfx/wide");
    let mut input = b"A/pfx/lib\0".to_vec();
    assert_eq!(input.len(), 10);
    input.extend_from_slice(&wide);
    input.extend_from_slice(&[0, 0]);
    input.extend_from_slice(b"tail");

    let groups = [
        OffsetGroup {
            encoding: OffsetEncoding::Utf16Le,
            ranges: OffsetRanges::Binary(vec![vec![10, 28]]),
            unknown_members: vec![],
        },
        utf8_group(OffsetRanges::Binary(vec![vec![1, 9]])),
    ];

    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_placeholders_with_offsets(
        &input,
        &mut output,
        placeholder,
        target,
        &Platform::Linux64,
        super::FileMode::Binary,
        &groups,
        None,
    )
    .unwrap();

    let out = output.into_inner();
    assert_eq!(out.len(), input.len(), "length must be preserved");
    // The UTF-8 c-string is patched, with padding restoring its length.
    assert_eq!(&out[..10], b"A/np/lib\0\0");
    // The wide string is patched under its own encoding, padded with a zero code unit.
    let mut expected_wide = encode(&OffsetEncoding::Utf16Le, "/np/wide");
    expected_wide.extend_from_slice(&[0, 0]);
    assert_eq!(&out[10..28], expected_wide.as_slice());
    assert_eq!(&out[28..], &input[28..], "the tail is copied verbatim");

    // The search-based path must produce exactly the same bytes.
    let mut searched = Cursor::new(Vec::new());
    super::copy_and_replace_cstring_placeholder(&input, &mut searched, placeholder, target)
        .unwrap();
    assert_eq!(searched.into_inner(), out);
}

/// A wide-string occurrence is replaced by the search-based and the offsets-based path alike,
/// under every encoding the draft CEP defines, in text and in binary files.
#[rstest]
#[case(OffsetEncoding::Utf8)]
#[case(OffsetEncoding::Utf16Le)]
#[case(OffsetEncoding::Utf16Be)]
#[case(OffsetEncoding::Utf32Le)]
#[case(OffsetEncoding::Utf32Be)]
fn test_offsets_and_search_agree_per_encoding(#[case] encoding: OffsetEncoding) {
    let placeholder = "/placeholder";
    let target = "/tgt";
    let unit = encoding.code_unit_size().unwrap();

    // `head` + the encoded string + its NUL terminator + a tail.
    let encoded = encode(&encoding, "/placeholder/lib");
    let offset = 8;
    let nul_pos = offset + encoded.len();
    let mut input = b"headhead".to_vec();
    input.extend_from_slice(&encoded);
    input.extend(std::iter::repeat_n(0u8, unit));
    input.extend_from_slice(b"tail");

    // Binary: the offsets path and the search must agree and preserve the length.
    let groups = [OffsetGroup {
        encoding: encoding.clone(),
        ranges: OffsetRanges::Binary(vec![vec![offset, nul_pos]]),
        unknown_members: vec![],
    }];
    let mut spliced = Cursor::new(Vec::new());
    super::copy_and_replace_cstring_placeholder_offsets(
        &input,
        &mut spliced,
        placeholder,
        target,
        &groups,
    )
    .unwrap();
    let spliced = spliced.into_inner();

    let mut searched = Cursor::new(Vec::new());
    super::copy_and_replace_cstring_placeholder(&input, &mut searched, placeholder, target)
        .unwrap();
    assert_eq!(spliced, searched.into_inner(), "binary paths must agree");
    assert_eq!(spliced.len(), input.len(), "length must be preserved");

    let freed = (placeholder.len() - target.len()) * unit;
    let mut expected = b"headhead".to_vec();
    expected.extend_from_slice(&encode(&encoding, "/tgt/lib"));
    // The bytes the shorter target frees up are zeroed, then the original terminator and the
    // tail follow.
    expected.extend(std::iter::repeat_n(0u8, freed));
    expected.extend(std::iter::repeat_n(0u8, unit));
    expected.extend_from_slice(b"tail");
    assert_eq!(spliced, expected);

    // Text: the same occurrence is replaced without padding, and both paths agree.
    let groups = [OffsetGroup {
        encoding: encoding.clone(),
        ranges: OffsetRanges::Text(vec![offset]),
        unknown_members: vec![],
    }];
    let mut spliced = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder_offsets(
        &input,
        &mut spliced,
        placeholder,
        target,
        &Platform::Linux64,
        &groups,
        None,
    )
    .unwrap();
    let spliced = spliced.into_inner();

    let mut searched = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder(
        &input,
        &mut searched,
        placeholder,
        target,
        &Platform::Linux64,
    )
    .unwrap();
    assert_eq!(spliced, searched.into_inner(), "text paths must agree");
    assert_eq!(
        spliced.len(),
        input.len() - (placeholder.len() - target.len()) * unit
    );
}

/// On a target without shebang handling the shebang region gets plain replacement, which must
/// cover every encoding just like the body does. A wide-encoded occurrence in the first line
/// is excluded from `offsets` by a conformant producer, so only the region replacement can
/// patch it, and the search-based path patches it because it treats the whole file uniformly.
#[test]
fn test_shebang_region_replaces_every_encoding_on_non_unix() {
    let placeholder = "/pfx";
    let target = "/t";

    let mut input = b"#!/pfx/python ".to_vec();
    input.extend_from_slice(&encode(&OffsetEncoding::Utf16Le, placeholder));
    input.extend_from_slice(b"\nbody /pfx\n");
    // The first newline sits at 22, so the region is the first 23 bytes and the only body
    // occurrence is the UTF-8 one at 28.
    assert_eq!(input.iter().position(|&c| c == b'\n'), Some(22));
    let groups = utf8_text_groups(&[28]);

    let mut spliced = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder_offsets(
        &input,
        &mut spliced,
        placeholder,
        target,
        &Platform::Win64,
        &groups,
        Some(23),
    )
    .unwrap();
    let spliced = spliced.into_inner();

    let mut expected = b"#!/t/python ".to_vec();
    expected.extend_from_slice(&encode(&OffsetEncoding::Utf16Le, target));
    expected.extend_from_slice(b"\nbody /t\n");
    assert_eq!(spliced, expected);

    let mut searched = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder(
        &input,
        &mut searched,
        placeholder,
        target,
        &Platform::Win64,
    )
    .unwrap();
    assert_eq!(searched.into_inner(), spliced, "both paths must agree");
}

/// An empty `prefix_placeholder` means there is nothing to replace. An empty needle matches at
/// every byte, so searching for it would insert the target prefix between all of them; both
/// paths must copy the file verbatim instead.
#[rstest]
#[case(Platform::Linux64)]
#[case(Platform::Win64)]
fn test_empty_placeholder_copies_verbatim(#[case] platform: Platform) {
    let input = b"#!/bin/python\nbody\n";

    let mut spliced = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder_offsets(
        input,
        &mut spliced,
        "",
        "/target",
        &platform,
        &[],
        Some(14),
    )
    .unwrap();
    assert_eq!(spliced.into_inner(), input);

    let mut searched = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder(input, &mut searched, "", "/target", &platform)
        .unwrap();
    assert_eq!(searched.into_inner(), input);
}

/// The placeholder and the target prefix have different lengths per encoding once the
/// placeholder leaves ASCII. Only the encodings that actually occur in the file may fail the
/// install: here the UTF-8 replacement fits and the wide ones (which the file does not use)
/// do not.
#[test]
fn test_binary_growing_prefix_only_rejected_for_encodings_in_use() {
    let placeholder = "/\u{e9}";
    let target = "/ab";
    assert_eq!(placeholder.len(), target.len());
    assert!(
        encode(&OffsetEncoding::Utf16Le, target).len()
            > encode(&OffsetEncoding::Utf16Le, placeholder).len()
    );

    let mut input = b"x".to_vec();
    input.extend_from_slice(placeholder.as_bytes());
    input.extend_from_slice(b"/lib\0");

    let mut searched = Cursor::new(Vec::new());
    super::copy_and_replace_cstring_placeholder(&input, &mut searched, placeholder, target)
        .unwrap();
    let searched = searched.into_inner();
    assert_eq!(searched, b"x/ab/lib\0");

    let groups = utf8_binary_groups(&[vec![1, 8]]);
    let mut spliced = Cursor::new(Vec::new());
    super::copy_and_replace_cstring_placeholder_offsets(
        &input,
        &mut spliced,
        placeholder,
        target,
        &groups,
    )
    .unwrap();
    assert_eq!(spliced.into_inner(), searched, "both paths must agree");
}

/// A target prefix that does not fit under an encoding the file does use still fails, because
/// binary replacement cannot grow the file.
#[test]
fn test_binary_growing_prefix_rejected_for_encoding_in_use() {
    let mut input = encode(&OffsetEncoding::Utf16Le, "/\u{e9}/lib");
    input.extend_from_slice(&[0, 0]);

    let mut out = Cursor::new(Vec::new());
    let result = super::copy_and_replace_cstring_placeholder(&input, &mut out, "/\u{e9}", "/ab");
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
}

/// The encodings of an ASCII placeholder are byte-shifted variants of one another, so a
/// genuine UTF-16-LE occurrence preceded by a zero byte also matches the UTF-16-BE needle one
/// byte earlier. That spurious candidate is misaligned, its terminator scan runs off the end
/// of the file, and applying it would both shift every following byte and leave the UTF-8
/// occurrence that follows unpatched.
#[test]
fn test_binary_shifted_cross_encoding_match_does_not_swallow_later_cstrings() {
    let placeholder = "/pfx";
    let target = "/p";

    let mut input = vec![0u8, 0]; // padding in front of the wide string
    input.extend_from_slice(&encode(&OffsetEncoding::Utf16Le, placeholder)); // 2..10
    input.extend_from_slice(&encode(&OffsetEncoding::Utf16Le, "\u{20ac}")); // 10..12
    input.extend_from_slice(&[0, 0]); // 12..14, the wide terminator
    input.extend_from_slice(b"/pfx\0"); // 14..19, an ordinary c-string

    let mut searched = Cursor::new(Vec::new());
    super::copy_and_replace_cstring_placeholder(&input, &mut searched, placeholder, target)
        .unwrap();
    let searched = searched.into_inner();

    let mut expected = vec![0u8, 0];
    expected.extend_from_slice(&encode(&OffsetEncoding::Utf16Le, target));
    expected.extend_from_slice(&encode(&OffsetEncoding::Utf16Le, "\u{20ac}"));
    expected.extend_from_slice(&[0, 0, 0, 0]); // the freed code units
    expected.extend_from_slice(&[0, 0]); // the wide terminator
    expected.extend_from_slice(b"/p\0\0\0"); // the c-string, padded
    assert_eq!(searched, expected);
    assert_eq!(searched.len(), input.len());

    // Identical to what the metadata a producer records splices.
    let groups = [
        OffsetGroup {
            encoding: OffsetEncoding::Utf16Le,
            ranges: OffsetRanges::Binary(vec![vec![2, 12]]),
            unknown_members: vec![],
        },
        utf8_group(OffsetRanges::Binary(vec![vec![14, 18]])),
    ];
    let mut spliced = Cursor::new(Vec::new());
    super::copy_and_replace_cstring_placeholder_offsets(
        &input,
        &mut spliced,
        placeholder,
        target,
        &groups,
    )
    .unwrap();
    assert_eq!(spliced.into_inner(), searched, "both paths must agree");
}

/// The same shifted-sibling ambiguity with a target prefix outside Latin-1, where picking the
/// wrong encoding is not merely a shifted write but byte-swapped garbage: UTF-16-BE of the
/// target is not UTF-16-LE of the target shifted by one.
#[test]
fn test_binary_wide_string_is_patched_with_its_own_encoding() {
    let placeholder = "/pfx";
    let target = "/\u{20ac}";

    let mut input = vec![0u8, 0];
    input.extend_from_slice(&encode(&OffsetEncoding::Utf16Le, placeholder));
    input.extend_from_slice(&[0, 0]);

    let mut searched = Cursor::new(Vec::new());
    super::copy_and_replace_cstring_placeholder(&input, &mut searched, placeholder, target)
        .unwrap();
    let searched = searched.into_inner();

    let mut expected = vec![0u8, 0];
    expected.extend_from_slice(&encode(&OffsetEncoding::Utf16Le, target));
    expected.extend_from_slice(&[0, 0, 0, 0]); // the two freed code units
    expected.extend_from_slice(&[0, 0]); // the wide terminator
    assert_eq!(searched, expected);

    let groups = [OffsetGroup {
        encoding: OffsetEncoding::Utf16Le,
        ranges: OffsetRanges::Binary(vec![vec![2, 10]]),
        unknown_members: vec![],
    }];
    let mut spliced = Cursor::new(Vec::new());
    super::copy_and_replace_cstring_placeholder_offsets(
        &input,
        &mut spliced,
        placeholder,
        target,
        &groups,
    )
    .unwrap();
    assert_eq!(spliced.into_inner(), searched, "both paths must agree");
}

/// Two occurrences of the same wide encoding at different byte parities cannot both be
/// code-unit aligned strings, so only the aligned one is patched. Metadata that claims they
/// share a c-string is rejected, which sends the installer to the search path and therefore to
/// the same bytes.
#[test]
fn test_binary_same_encoding_at_two_parities() {
    let placeholder = "/pfx";
    let target = "/p";

    let mut input = encode(&OffsetEncoding::Utf16Le, placeholder); // 0..8, aligned
    input.push(b'A'); // 8, shifts what follows to an odd offset
    input.extend_from_slice(&encode(&OffsetEncoding::Utf16Le, placeholder)); // 9..17
    input.extend_from_slice(&[0, 0]); // 17..19

    let mut searched = Cursor::new(Vec::new());
    super::copy_and_replace_cstring_placeholder(&input, &mut searched, placeholder, target)
        .unwrap();
    let searched = searched.into_inner();
    assert_eq!(searched.len(), input.len());
    // The aligned occurrence is replaced; the misaligned one keeps its placeholder rather than
    // being patched through a c-string with the wrong terminator.
    assert_eq!(&searched[..4], encode(&OffsetEncoding::Utf16Le, target));

    let groups = [OffsetGroup {
        encoding: OffsetEncoding::Utf16Le,
        ranges: OffsetRanges::Binary(vec![vec![0, 9, 17]]),
        unknown_members: vec![],
    }];
    let mut spliced = Cursor::new(Vec::new());
    let result = super::copy_and_replace_cstring_placeholder_offsets(
        &input,
        &mut spliced,
        placeholder,
        target,
        &groups,
    );
    assert!(
        matches!(
            result,
            Err(super::OffsetReplaceError::InconsistentMetadata(_))
        ),
        "{result:?}"
    );
    assert!(spliced.into_inner().is_empty());
}

/// A recorded terminator must really be a zero code unit (or end-of-file). Padding written
/// into the middle of a live c-string would truncate it and strand its tail.
#[rstest]
#[case::inside_a_live_string(vec![vec![0, 7]])]
#[case::past_the_end(vec![vec![0, 12]])]
fn test_binary_recorded_nul_must_be_a_zero_code_unit(#[case] cstrings: Vec<Vec<usize>>) {
    let input = b"/pfxABCDEF\0";
    let mut output = Cursor::new(Vec::new());
    let result = super::copy_and_replace_cstring_placeholder_offsets(
        input,
        &mut output,
        "/pfx",
        "/p",
        &utf8_binary_groups(&cstrings),
    );
    assert!(
        matches!(
            result,
            Err(super::OffsetReplaceError::InconsistentMetadata(_))
        ),
        "{result:?}"
    );
    assert!(output.into_inner().is_empty());
}

/// The documented shape of the binary ranges, several occurrences spread over two c-strings
/// with their own terminators, must be applicable.
#[test]
fn test_binary_multiple_occurrences_in_two_cstrings() {
    let mut input = vec![b'A'; 40];
    input[5..9].copy_from_slice(b"/pfx");
    input[19] = 0;
    input[22..26].copy_from_slice(b"/pfx");
    input[30..34].copy_from_slice(b"/pfx");
    input[39] = 0;

    let groups = utf8_binary_groups(&[vec![5, 19], vec![22, 30, 39]]);
    let mut spliced = Cursor::new(Vec::new());
    super::copy_and_replace_cstring_placeholder_offsets(
        &input,
        &mut spliced,
        "/pfx",
        "/p",
        &groups,
    )
    .unwrap();
    let spliced = spliced.into_inner();
    assert_eq!(spliced.len(), input.len());

    let mut searched = Cursor::new(Vec::new());
    super::copy_and_replace_cstring_placeholder(&input, &mut searched, "/pfx", "/p").unwrap();
    assert_eq!(spliced, searched.into_inner(), "both paths must agree");
    assert!(
        memchr::memmem::find(&spliced, b"/pfx").is_none(),
        "every occurrence must be replaced"
    );
}

/// A group for an encoding under which the file has no occurrence is metadata that does not
/// match the contents: the installer falls back to searching rather than writing garbage.
#[test]
fn test_offset_groups_encoding_without_occurrence_is_inconsistent() {
    let input = b"no utf-8 occurrences here";
    for (file_mode, ranges) in [
        (
            super::FileMode::Binary,
            OffsetRanges::Binary(vec![vec![10, 24]]),
        ),
        (super::FileMode::Text, OffsetRanges::Text(vec![10])),
    ] {
        let groups = [OffsetGroup {
            encoding: OffsetEncoding::Utf16Le,
            ranges,
            unknown_members: vec![],
        }];
        let mut output = Cursor::new(Vec::new());
        let result = super::copy_and_replace_placeholders_with_offsets(
            input,
            &mut output,
            "/pfx",
            "/np",
            &Platform::Linux64,
            file_mode,
            &groups,
            None,
        );
        assert!(
            matches!(
                result,
                Err(super::OffsetReplaceError::InconsistentMetadata(_))
            ),
            "mode {file_mode:?}"
        );
        assert!(output.into_inner().is_empty(), "mode {file_mode:?}");
    }
}

/// Structurally invalid group lists (an unrecognized encoding, duplicate
/// encodings, or an empty list for a binary file) surface as inconsistent
/// metadata (with nothing written) so the installer falls back to the
/// search-based replacement.
#[rstest]
#[case::unknown_encoding(vec![OffsetGroup {
    encoding: OffsetEncoding::Unknown(String::from("utf-64-xe")),
    ranges: OffsetRanges::Binary(vec![vec![1, 9]]),
    unknown_members: vec![],
}])]
#[case::duplicate_encoding(vec![
    utf8_group(OffsetRanges::Binary(vec![vec![1, 9]])),
    utf8_group(OffsetRanges::Binary(vec![vec![1, 9]])),
])]
#[case::empty_list(vec![])]
fn test_offset_groups_invalid_is_inconsistent(#[case] groups: Vec<OffsetGroup>) {
    let mut output = Cursor::new(Vec::new());
    let result = super::copy_and_replace_placeholders_with_offsets(
        b"A/pfx/lib\0",
        &mut output,
        "/pfx",
        "/np",
        &Platform::Linux64,
        super::FileMode::Binary,
        &groups,
        None,
    );
    assert!(
        matches!(
            result,
            Err(super::OffsetReplaceError::InconsistentMetadata(_))
        ),
        "{result:?}"
    );
    assert!(
        output.into_inner().is_empty(),
        "nothing is written, so the fallback starts from a clean destination"
    );
}

/// Offsets come from the (untrusted) `paths.json`. Malformed offsets must return a recoverable
/// error rather than panic and take down the caller (e.g. a FUSE/NFS read thread).
#[rstest]
// Offset past the end of the file.
#[case(vec![1000])]
// Offsets out of order (second starts before the first prefix ends).
#[case(vec![7, 0])]
fn test_textual_offsets_invalid_returns_error(#[case] offsets: Vec<usize>) {
    let mut output = Cursor::new(Vec::new());
    let result = super::copy_and_replace_textual_placeholder_offsets(
        b"Hello, cruel world!",
        &mut output,
        "cruel",
        "fabulous",
        &Platform::Linux64,
        &utf8_text_groups(&offsets),
        None,
    );
    assert!(
        matches!(
            result,
            Err(super::OffsetReplaceError::InconsistentMetadata(_))
        ),
        "malformed offsets should surface as inconsistent metadata, not a panic: {result:?}"
    );
    // Nothing must be written when the metadata is rejected, so the caller can reuse the
    // destination for search-based replacement.
    assert!(output.into_inner().is_empty());
}

/// Malformed binary offset groups must also return an error rather than panic (empty group,
/// out-of-range NUL position, ...).
#[rstest]
// Empty group would underflow `group.len() - 1`.
#[case(vec![vec![]])]
// Prefix offset and NUL position beyond the end of the file.
#[case(vec![vec![1000, 2000]])]
fn test_binary_offsets_invalid_returns_error(#[case] groups: Vec<Vec<usize>>) {
    let mut output = Cursor::new(Vec::new());
    let result = super::copy_and_replace_cstring_placeholder_offsets(
        b"12345Hello, fabulous world!\x006789",
        &mut output,
        "fabulous",
        "cruel",
        &utf8_binary_groups(&groups),
    );
    assert!(
        matches!(
            result,
            Err(super::OffsetReplaceError::InconsistentMetadata(_))
        ),
        "malformed offsets should surface as inconsistent metadata, not a panic: {result:?}"
    );
    // Nothing must be written when the metadata is rejected.
    assert!(output.into_inner().is_empty());
}

#[rstest]
// The NUL terminator sits at offset 27 (the `\x00` byte), not 28. The last value of the group
// is the NUL position, per the draft CEP.
#[case(
    b"12345Hello, fabulous world!\x006789",
    vec![vec![12, 27]],
    "fabulous",
    "cruel",
    b"12345Hello, cruel world!\x00\x00\x00\x006789"
)]
pub fn test_copy_and_replace_binary_placeholder_offsets(
    #[case] input: &[u8],
    #[case] groups: Vec<Vec<usize>>,
    #[case] prefix_placeholder: &str,
    #[case] target_prefix: &str,
    #[case] expected_output: &[u8],
) {
    assert_eq!(
        expected_output.len(),
        input.len(),
        "input and expected output must have the same length"
    );
    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_cstring_placeholder_offsets(
        input,
        &mut output,
        prefix_placeholder,
        target_prefix,
        &utf8_binary_groups(&groups),
    )
    .unwrap();
    assert_eq!(&output.into_inner(), expected_output);
}

#[rstest]
#[case(b"short\x00", vec![vec![0, 5]], "short", "verylong")]
#[case(b"short1234\x00", vec![vec![0, 9]], "short", "verylong")]
pub fn test_shorter_binary_placeholder_offsets(
    #[case] input: &[u8],
    #[case] groups: Vec<Vec<usize>>,
    #[case] prefix_placeholder: &str,
    #[case] target_prefix: &str,
) {
    assert!(target_prefix.len() > prefix_placeholder.len());

    let mut output = Cursor::new(Vec::new());
    let result = super::copy_and_replace_cstring_placeholder_offsets(
        input,
        &mut output,
        prefix_placeholder,
        target_prefix,
        &utf8_binary_groups(&groups),
    );
    assert!(result.is_err());
}

#[rstest]
#[case(
    b"beginrandomdataPATH=/placeholder/etc/share:/placeholder/bin/:\x00somemoretext",
    b"beginrandomdataPATH=/target/etc/share:/target/bin/:\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00somemoretext",
    vec![vec![20, 43, 61]]
)]
#[case(
    b"beginrandomdataPATH=/placeholder/etc/share:/placeholder/bin/another/placeholder/:\x00somemoretext",
    b"beginrandomdataPATH=/target/etc/share:/target/bin/another/target/:\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00somemoretext",
    vec![vec![20, 43, 67, 81]],
)]
fn replace_binary_path_var_offsets(
    #[case] input: &[u8],
    #[case] result: &[u8],
    #[case] groups: Vec<Vec<usize>>,
) {
    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_cstring_placeholder_offsets(
        input,
        &mut output,
        "/placeholder",
        "/target",
        &utf8_binary_groups(&groups),
    )
    .unwrap();
    let out = &output.into_inner();
    assert_eq!(out, result);
    assert_eq!(out.len(), input.len());
}

/// The placeholder occurs only inside the shebang line, so a conformant producer records
/// `offsets: []`. With a target prefix well over the 127-byte Linux limit the patched shebang
/// collapses to the `#!/usr/bin/env <program>` form.
#[test]
fn test_replace_long_prefix_in_text_file_offsets() {
    let test_data_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data");
    let test_file = test_data_dir.join("shebang_test.txt");
    let prefix_placeholder = "/this/is/placeholder";
    let mut target_prefix = "/super/long/".to_string();
    for _ in 0..15 {
        target_prefix.push_str("verylongstring/");
    }
    let input = fs::read(test_file).unwrap();

    // The only occurrence is inside the shebang region, so `offsets` is empty.
    // Derive `shebang_length` (first-newline index + 1) from the file rather than
    // hardcoding it, so the test is robust to a CRLF checkout on Windows, where the
    // extra carriage return shifts the newline and thus the region length.
    let offsets: Vec<usize> = Vec::new();
    let shebang_length = input
        .iter()
        .position(|&c| c == b'\n')
        .map_or(input.len(), |i| i + 1);

    let mut output = Cursor::new(Vec::new());
    super::copy_and_replace_textual_placeholder_offsets(
        &input,
        &mut output,
        prefix_placeholder,
        &target_prefix,
        &Platform::Linux64,
        &utf8_text_groups(&offsets),
        Some(shebang_length),
    )
    .unwrap();

    let output = output.into_inner();
    let replaced = String::from_utf8_lossy(&output);
    insta::assert_snapshot!(replaced);
}
