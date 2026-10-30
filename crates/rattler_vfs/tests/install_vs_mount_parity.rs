//! Install-vs-mount byte parity tests.
//!
//! These tests verify that `rattler::install::link`'s prefix-replacement
//! routines (which write transformed bytes to a destination Writer) and
//! `rattler_vfs::prefix_replacement`'s ranged-read routines (which return
//! transformed bytes from a source slice) produce **byte-identical** output
//! for the same input.
//!
//! This catches drift between the install-time and mount-time prefix
//! replacement code paths — the same package on disk vs. mounted should be
//! indistinguishable. Shebang scripts are covered explicitly, since the
//! installer rewrites the first line (and may collapse an over-long one to
//! `#!/usr/bin/env <program>`) while the body is spliced at offsets.

use std::io::Cursor;

use rattler_conda_types::{
    Subdir,
    package::{FileMode, OffsetEncoding},
};
use rattler_vfs::prefix_replacement::{
    EncodedPrefixes, binary_ranged_read, plan_binary_replacement, plan_text_replacement,
    text_ranged_read,
};

/// Run install-time prefix replacement and return the resulting bytes.
fn install_replace(
    source: &[u8],
    placeholder: &str,
    target_prefix: &str,
    file_mode: FileMode,
    platform: Subdir,
) -> Vec<u8> {
    let mut output = Cursor::new(Vec::<u8>::new());
    rattler::install::link::copy_and_replace_placeholders(
        source,
        &mut output,
        placeholder,
        target_prefix,
        &platform,
        file_mode,
    )
    .expect("install-time replacement should succeed");
    output.into_inner()
}

/// Run mount-time ranged-read replacement over `[start, end)` of the output,
/// mirroring what the VFS serves for that read.
fn mount_replace_range(
    source: &[u8],
    placeholder: &str,
    target: &str,
    file_mode: FileMode,
    platform: Subdir,
    start: usize,
    end: usize,
) -> Vec<u8> {
    let prefixes = EncodedPrefixes::new(placeholder, target);
    match file_mode {
        FileMode::Text => {
            let plan = plan_text_replacement(source, placeholder, target, &platform);
            text_ranged_read(source, &prefixes, &plan, start, end)
        }
        FileMode::Binary => {
            let cstrings = plan_binary_replacement(source, placeholder);
            binary_ranged_read(source, &prefixes, &cstrings, start, end)
        }
    }
}

/// Run mount-time ranged-read replacement over the full output range, mirroring
/// what the VFS serves for a whole-file read.
fn mount_replace_full(
    source: &[u8],
    placeholder: &str,
    target: &str,
    file_mode: FileMode,
    platform: Subdir,
) -> Vec<u8> {
    mount_replace_range(
        source,
        placeholder,
        target,
        file_mode,
        platform,
        0,
        usize::MAX,
    )
}

/// Assert install-time and mount-time full replacement agree byte-for-byte.
fn assert_full_parity(
    source: &[u8],
    placeholder: &str,
    target: &str,
    file_mode: FileMode,
    platform: Subdir,
) {
    let install = install_replace(source, placeholder, target, file_mode, platform);
    let mount = mount_replace_full(source, placeholder, target, file_mode, platform);
    assert_eq!(
        install, mount,
        "install vs mount diverged (mode={file_mode:?}, platform={platform}) for {source:?}"
    );
}

/// Assert that each windowed mount read matches the corresponding slice of the
/// full install output.
fn assert_ranged_parity(
    source: &[u8],
    placeholder: &str,
    target: &str,
    file_mode: FileMode,
    platform: Subdir,
    ranges: &[(usize, usize)],
) {
    let install = install_replace(source, placeholder, target, file_mode, platform);
    for &(start, end) in ranges {
        let mount_slice =
            mount_replace_range(source, placeholder, target, file_mode, platform, start, end);
        let expected = &install[start.min(install.len())..end.min(install.len())];
        assert_eq!(
            mount_slice, expected,
            "ranged read [{start}, {end}) diverged (mode={file_mode:?}, platform={platform})"
        );
    }
}

// ---------------------------------------------------------------------------
// Text mode parity (no shebang)
// ---------------------------------------------------------------------------

#[test]
fn text_mode_simple_replacement_matches_install() {
    let placeholder = "/old/conda/prefix";
    let target = "/new/longer/conda/prefix";
    let source = format!("hello {placeholder} world\n");
    assert_full_parity(
        source.as_bytes(),
        placeholder,
        target,
        FileMode::Text,
        Subdir::Linux64,
    );
}

#[test]
fn text_mode_multiple_replacements_match_install() {
    // Three placeholders separated by literal text.
    assert_full_parity(
        b"a/p b/p c/p d",
        "/p",
        "/QQQQ",
        FileMode::Text,
        Subdir::Linux64,
    );
}

#[test]
fn text_mode_no_replacement_match_install() {
    assert_full_parity(
        b"completely unrelated content with no placeholder\n",
        "/old/conda/prefix",
        "/new/conda/prefix",
        FileMode::Text,
        Subdir::Linux64,
    );
}

#[test]
fn text_mode_shorter_target_matches_install() {
    let placeholder = "/long/old/prefix/path";
    let source = format!("{placeholder}/bin/python\n");
    assert_full_parity(
        source.as_bytes(),
        placeholder,
        "/short",
        FileMode::Text,
        Subdir::Linux64,
    );
}

// ---------------------------------------------------------------------------
// Shebang parity — the installer rewrites the first line; the mount path must
// reproduce those exact bytes.
// ---------------------------------------------------------------------------

#[test]
fn shebang_kept_short_prefix_matches_install() {
    let placeholder = "/opt/old/prefix";
    let target = "/opt/new";
    let source = format!("#!{placeholder}/bin/python\nimport os  # {placeholder}/lib\n");
    let bytes = source.into_bytes();
    assert_full_parity(&bytes, placeholder, target, FileMode::Text, Subdir::Linux64);
    assert_ranged_parity(
        &bytes,
        placeholder,
        target,
        FileMode::Text,
        Subdir::Linux64,
        &[(0, 3), (2, 25), (10, 40), (0, 4096), (35, 4096)],
    );
}

#[test]
fn shebang_collapses_long_prefix_matches_install() {
    // A target well over the 127-byte Linux limit forces the first line to
    // collapse to `#!/usr/bin/env <program>`.
    let placeholder = "/opt/old";
    let mut target = String::from("/opt");
    for _ in 0..20 {
        target.push_str("/verylongsegment");
    }
    assert!(target.len() > 127);
    let source = format!("#!{placeholder}/bin/perl\nprint 1;\n");
    assert_full_parity(
        source.as_bytes(),
        placeholder,
        &target,
        FileMode::Text,
        Subdir::Linux64,
    );
}

#[test]
fn shebang_no_trailing_newline_matches_install() {
    let placeholder = "/opt/old/prefix";
    let source = format!("#!{placeholder}/bin/python");
    assert_full_parity(
        source.as_bytes(),
        placeholder,
        "/opt/new",
        FileMode::Text,
        Subdir::Linux64,
    );
}

#[test]
fn shebang_multiple_occurrences_in_line_matches_install() {
    let placeholder = "/opt/old";
    let source = format!("#!{placeholder}/bin/python -S {placeholder}/site\nx = 1\n");
    assert_full_parity(
        source.as_bytes(),
        placeholder,
        "/opt/new",
        FileMode::Text,
        Subdir::Linux64,
    );
}

#[test]
fn shebang_only_occurrence_in_line_matches_install() {
    let placeholder = "/opt/old/prefix";
    let source = format!("#!{placeholder}/bin/python\nimport os\n");
    assert_full_parity(
        source.as_bytes(),
        placeholder,
        "/opt/new",
        FileMode::Text,
        Subdir::Linux64,
    );
}

#[test]
fn shebang_non_rewriting_target_matches_install() {
    // On a non-Unix target (e.g. a noarch package mounted on Windows) there is
    // no shebang machinery: the region gets plain placeholder replacement, like
    // the body.
    let placeholder = "/opt/old";
    let target = "/opt/new";
    let source = format!("#!{placeholder}/bin/python\nimport os  # {placeholder}/lib\n");
    let bytes = source.into_bytes();
    assert_full_parity(&bytes, placeholder, target, FileMode::Text, Subdir::Win64);
    assert_ranged_parity(
        &bytes,
        placeholder,
        target,
        FileMode::Text,
        Subdir::Win64,
        &[(0, 5), (2, 30), (0, 4096)],
    );
}

// ---------------------------------------------------------------------------
// Binary mode parity (c-string with null terminator and padding)
// ---------------------------------------------------------------------------

/// Build a binary blob containing a c-string with a placeholder, terminated
/// by a null byte.
fn build_cstring(placeholder: &str, suffix: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(placeholder.as_bytes());
    buf.extend_from_slice(suffix.as_bytes());
    buf.push(0u8);
    // Some trailing context so the test catches replacement past the null.
    buf.extend_from_slice(b"\x01\x02\x03tail\x00");
    buf
}

#[test]
fn binary_mode_cstring_with_padding_matches_install() {
    let source = build_cstring("/long/old/prefix", "/lib/foo.so");
    assert_full_parity(
        &source,
        "/long/old/prefix",
        "/short",
        FileMode::Binary,
        Subdir::Linux64,
    );
}

#[test]
fn binary_mode_no_replacement_matches_install() {
    assert_full_parity(
        b"\x7fELF unrelated binary contents\x00\x01\x02\x00",
        "/long/old/prefix",
        "/short",
        FileMode::Binary,
        Subdir::Linux64,
    );
}

#[test]
fn binary_mode_multiple_cstrings_match_install() {
    let placeholder = "/long/old/prefix";
    let mut source = Vec::new();
    source.extend_from_slice(placeholder.as_bytes());
    source.extend_from_slice(b"/a\x00");
    source.extend_from_slice(placeholder.as_bytes());
    source.extend_from_slice(b"/b\x00");
    source.extend_from_slice(b"unrelated\x00");
    assert_full_parity(
        &source,
        placeholder,
        "/p",
        FileMode::Binary,
        Subdir::Linux64,
    );
}

// ---------------------------------------------------------------------------
// Ranged-read parity: mount-time read of an arbitrary slice should equal the
// corresponding slice of the install-time output.
// ---------------------------------------------------------------------------

#[test]
fn ranged_read_text_matches_install_slice() {
    let placeholder = "/old/conda/prefix";
    let target = "/new/longer/conda/prefix";
    let source = format!("hello {placeholder} middle {placeholder} tail\n");
    let install_len = install_replace(
        source.as_bytes(),
        placeholder,
        target,
        FileMode::Text,
        Subdir::Linux64,
    )
    .len();
    assert_ranged_parity(
        source.as_bytes(),
        placeholder,
        target,
        FileMode::Text,
        Subdir::Linux64,
        &[(0, 5), (3, 20), (7, install_len), (0, 1)],
    );
}

#[test]
fn ranged_read_binary_matches_install_slice() {
    let source = build_cstring("/long/old/prefix", "/bin/foo");
    let install_len = install_replace(
        &source,
        "/long/old/prefix",
        "/p",
        FileMode::Binary,
        Subdir::Linux64,
    )
    .len();
    assert_ranged_parity(
        &source,
        "/long/old/prefix",
        "/p",
        FileMode::Binary,
        Subdir::Linux64,
        &[(0, 4), (2, 16), (0, install_len), (10, 20)],
    );
}

// ---------------------------------------------------------------------------
// Wide encodings (UTF-16 / UTF-32): the installer replaces the placeholder in
// every encoding the CEP defines; the mount must produce the same bytes.
// ---------------------------------------------------------------------------

const WIDE: [OffsetEncoding; 4] = [
    OffsetEncoding::Utf16Le,
    OffsetEncoding::Utf16Be,
    OffsetEncoding::Utf32Le,
    OffsetEncoding::Utf32Be,
];

/// Every window of a few sizes, so seeking is exercised at every position.
fn all_windows(len: usize) -> Vec<(usize, usize)> {
    (0..len)
        .flat_map(|start| [1usize, 2, 5, 16].map(|n| (start, start + n)))
        .chain([(0, len), (0, usize::MAX)])
        .collect()
}

#[test]
fn text_mode_wide_encoded_file_matches_install() {
    let placeholder = "/old/conda/prefix";
    for encoding in WIDE {
        let source = encoding.encode(&format!("path={placeholder}/lib\nhome={placeholder}\n"));
        for target in ["/new/longer/conda/prefix", "/p"] {
            assert_full_parity(
                &source,
                placeholder,
                target,
                FileMode::Text,
                Subdir::Linux64,
            );
            assert_ranged_parity(
                &source,
                placeholder,
                target,
                FileMode::Text,
                Subdir::Linux64,
                &all_windows(source.len() + 64),
            );
        }
    }
}

#[test]
fn text_mode_mixed_encodings_match_install() {
    // A UTF-8 script carrying a UTF-16-LE resource string and a UTF-32-BE one,
    // under a shebang so the region handling is mixed in too.
    let placeholder = "/opt/old";
    let mut source = format!("#!{placeholder}/bin/python\nx = '{placeholder}'\n").into_bytes();
    source.resize(source.len().next_multiple_of(4), b' ');
    source.extend(OffsetEncoding::Utf16Le.encode(&format!("{placeholder}/share")));
    source.extend(OffsetEncoding::Utf32Be.encode(&format!("{placeholder}/etc")));
    source.extend_from_slice(format!("\ny = '{placeholder}'\n").as_bytes());

    for platform in [Subdir::Linux64, Subdir::Win64] {
        assert_full_parity(
            &source,
            placeholder,
            "/opt/new/dir",
            FileMode::Text,
            platform,
        );
        assert_ranged_parity(
            &source,
            placeholder,
            "/opt/new/dir",
            FileMode::Text,
            platform,
            &all_windows(source.len() + 64),
        );
    }
}

#[test]
fn text_mode_overlapping_wide_matches_match_install() {
    // In UTF-16-LE text an ASCII placeholder preceded by a zero byte also
    // matches as UTF-16-BE one byte earlier; the installer keeps the aligned
    // match, and so must the mount.
    let placeholder = "/pfx";
    let mut source = vec![b'a', 0];
    source.extend(OffsetEncoding::Utf16Le.encode(&format!("{placeholder}/x {placeholder}")));
    assert_full_parity(
        &source,
        placeholder,
        "/new",
        FileMode::Text,
        Subdir::Linux64,
    );
    assert_ranged_parity(
        &source,
        placeholder,
        "/new",
        FileMode::Text,
        Subdir::Linux64,
        &all_windows(source.len() + 8),
    );
}

#[test]
fn binary_mode_wide_cstrings_match_install() {
    let placeholder = "/long/old/prefix";
    for encoding in WIDE {
        let unit = encoding.code_unit_size();
        let mut source = b"\x7fELF header".to_vec();
        // Compilers emit wide strings code-unit aligned.
        source.resize(source.len().next_multiple_of(unit), 0);
        source.extend(encoding.encode(&format!("{placeholder}/lib:{placeholder}/bin")));
        source.extend(vec![0; unit]);
        source.extend_from_slice(b"\x01\x02tail\x00");
        assert_full_parity(
            &source,
            placeholder,
            "/short",
            FileMode::Binary,
            Subdir::Linux64,
        );
        assert_ranged_parity(
            &source,
            placeholder,
            "/short",
            FileMode::Binary,
            Subdir::Linux64,
            &all_windows(source.len()),
        );
    }
}

#[test]
fn binary_mode_mixed_encodings_match_install() {
    let placeholder = "/long/old/prefix";
    let mut source = Vec::new();
    source.extend_from_slice(format!("{placeholder}/a\0").as_bytes());
    source.resize(source.len().next_multiple_of(2), 0);
    source.extend(OffsetEncoding::Utf16Le.encode(&format!("{placeholder}/b")));
    source.extend([0, 0]);
    source.resize(source.len().next_multiple_of(4), 0);
    source.extend(OffsetEncoding::Utf32Le.encode(&format!("{placeholder}/c")));
    source.extend([0, 0, 0, 0]);
    source.extend_from_slice(b"unrelated\0");
    assert_full_parity(
        &source,
        placeholder,
        "/p",
        FileMode::Binary,
        Subdir::Linux64,
    );
    assert_ranged_parity(
        &source,
        placeholder,
        "/p",
        FileMode::Binary,
        Subdir::Linux64,
        &all_windows(source.len()),
    );
}
