//! `PrjFileNameCompare`-compatible file-name ordering for `ProjFS` enumeration.
//!
//! Windows Projected File System (`ProjFS`) requires the entries returned from a
//! directory-enumeration callback to be sorted in the exact order the
//! filesystem itself uses — the order defined by the Win32 `PrjFileNameCompare`
//! API. If a provider returns entries in any other order (for example plain
//! Rust `str`/UTF-8 byte order), `ProjFS`'s streaming enumeration can skip or
//! mis-report entries: it walks the provider's list assuming it is already in
//! `PrjFileNameCompare` order and stops matching as soon as that assumption is
//! violated. See issue #2581.
//!
//! `PrjFileNameCompare` performs a **case-insensitive, locale-independent
//! (ordinal) comparison of UTF-16 code units**, mirroring
//! `RtlCompareUnicodeString(.., CaseInSensitive = TRUE)` — the same ordering
//! NTFS uses for its directory index. This differs from Rust's `str::cmp`
//! (UTF-8 byte order) in two important ways:
//!
//! 1. **Case-insensitivity.** `str::cmp` orders every uppercase ASCII letter
//!    before every lowercase one (`'Z'` = 0x5A < `'a'` = 0x61), so it sorts
//!    `"Zebra" < "apple"`. `PrjFileNameCompare` upper-cases both operands
//!    first, so it sorts `"apple" < "Zebra"`.
//! 2. **Comparison unit.** The comparison is over upper-cased UTF-16 code
//!    units, not raw UTF-8 bytes. For the Basic Multilingual Plane, UTF-16
//!    code-unit order equals Unicode scalar order equals UTF-8 byte order, so
//!    this only diverges for supplementary-plane characters (encoded as
//!    surrogate pairs, code units 0xD800–0xDFFF, which sort *before* the
//!    0xE000–0xFFFF range in UTF-16 but *after* it in scalar/UTF-8 order).
//!
//! Because `ProjFS` is Windows-only, this comparator only ever *feeds* `ProjFS`
//! on Windows. It is nevertheless implemented as a platform-independent
//! pure-Rust function (no `windows` dependency, not `cfg(windows)`-gated) for
//! two reasons: it keeps the enumeration sort as a single, testable source of
//! truth that runs identically everywhere, and it lets the unit tests below run
//! on any host (including the macOS/Linux CI that cannot compile the `ProjFS`
//! adapter). A `#[cfg(windows)]` test asserts the pure-Rust ordering agrees
//! with the real `PrjFileNameCompare` on a Windows host.
//!
//! ## Known divergences from the real `PrjFileNameCompare`
//!
//! The upper-casing here uses Rust's Unicode simple (1:1) uppercase mapping,
//! restricted to mappings that stay within a single BMP code unit. The real
//! API uses Windows' internal NLS/`$UpCase` table, which:
//!
//! * can differ from the Unicode mapping for a small number of code points and
//!   may vary slightly between Windows versions;
//! * never *expands* a character during upper-casing (e.g. `'ß'` U+00DF stays
//!   `'ß'` rather than becoming `"SS"`). We match this by only applying
//!   mappings that produce exactly one BMP code unit and leaving everything
//!   else unchanged.
//!
//! For the ASCII-dominated filename space of conda packages (letters, digits,
//! `.`, `-`, `_`, `+`, `~`) the two orderings are identical.

use std::cmp::Ordering;

/// Upper-case a single UTF-16 code unit the way `PrjFileNameCompare` /
/// `RtlUpcaseUnicodeChar` conceptually does: a 1:1 map that never expands.
///
/// * ASCII `a`–`z` map to `A`–`Z` (fast path, and exact).
/// * Surrogate-range units (0xD800–0xDFFF) are returned unchanged — upper-casing
///   is applied per code unit and surrogate halves have no case.
/// * Other BMP units use Rust's Unicode simple uppercase, but only when it
///   yields exactly one code unit that still fits in the BMP; otherwise the
///   unit is left unchanged (matching NTFS, which never expands during upcase).
fn upcase_u16(unit: u16) -> u16 {
    if unit < 0x80 {
        // ASCII fast path.
        if (u16::from(b'a')..=u16::from(b'z')).contains(&unit) {
            return unit - 32;
        }
        return unit;
    }
    if (0xD800..=0xDFFF).contains(&unit) {
        // Lone surrogate code unit: no case mapping.
        return unit;
    }
    if let Some(c) = char::from_u32(u32::from(unit)) {
        let mut upper = c.to_uppercase();
        match (upper.next(), upper.next()) {
            // Exactly one resulting char, and it still fits in a single UTF-16
            // code unit (i.e. it is itself in the BMP). This rejects both
            // expansions (e.g. 'ß' -> "SS") and any mapping that would leave
            // the BMP, matching the non-expanding NTFS/ProjFS upcase.
            (Some(u), None) if u as u32 <= 0xFFFF => return u as u16,
            _ => return unit,
        }
    }
    unit
}

/// Compare two file names using `PrjFileNameCompare` semantics: a
/// case-insensitive, ordinal comparison over upper-cased UTF-16 code units.
///
/// This is the ordering `ProjFS` requires for directory-enumeration results.
/// See the [module docs](self) for the precise semantics and the known
/// divergences from the real Win32 API.
pub fn prj_file_name_compare(a: &str, b: &str) -> Ordering {
    let mut a_units = a.encode_utf16();
    let mut b_units = b.encode_utf16();
    loop {
        match (a_units.next(), b_units.next()) {
            (Some(x), Some(y)) => {
                let ord = upcase_u16(x).cmp(&upcase_u16(y));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            // The shorter name sorts first once it runs out of code units.
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => return Ordering::Equal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn case_insensitive_equality() {
        assert_eq!(prj_file_name_compare("README", "readme"), Ordering::Equal);
        assert_eq!(prj_file_name_compare("ReadMe", "rEADmE"), Ordering::Equal);
        assert_eq!(prj_file_name_compare("ABC", "abc"), Ordering::Equal);
        // Case-insensitive, so these are Equal even though str::cmp would not be.
        assert_ne!("README".cmp("readme"), Ordering::Equal);
    }

    #[test]
    fn case_insensitive_ordering_differs_from_str_cmp() {
        // str::cmp puts uppercase before lowercase: 'B' (0x42) < 'a' (0x61),
        // so "Banana" < "apple" in byte order.
        assert_eq!("Banana".cmp("apple"), Ordering::Less);
        // PrjFileNameCompare upcases first: "APPLE" < "BANANA".
        assert_eq!(prj_file_name_compare("apple", "Banana"), Ordering::Less);
        assert_eq!(prj_file_name_compare("Banana", "apple"), Ordering::Greater);

        // Classic case: "Zebra" vs "apple".
        assert_eq!("Zebra".cmp("apple"), Ordering::Less); // 'Z' < 'a'
        assert_eq!(prj_file_name_compare("Zebra", "apple"), Ordering::Greater);
    }

    #[test]
    fn underscore_vs_letters_diverges_from_str_cmp() {
        // '_' is 0x5F, which sits *between* uppercase 'Z' (0x5A) and lowercase
        // 'a' (0x61). This makes '_' vs a letter order flip under upcasing.
        //
        // str::cmp: '_' (0x5F) < 'a' (0x61)  => "_foo" < "afoo"
        assert_eq!("_foo".cmp("afoo"), Ordering::Less);
        // Prj: 'a' upcases to 'A' (0x41) < '_' (0x5F) => "afoo" < "_foo"
        assert_eq!(prj_file_name_compare("afoo", "_foo"), Ordering::Less);
        assert_eq!(prj_file_name_compare("_foo", "afoo"), Ordering::Greater);

        // But against an *uppercase* letter str::cmp already agrees with Prj,
        // since 'A' (0x41) < '_' (0x5F).
        assert_eq!("Afoo".cmp("_foo"), Ordering::Less);
        assert_eq!(prj_file_name_compare("Afoo", "_foo"), Ordering::Less);
    }

    #[test]
    fn separator_punctuation_is_ordinal() {
        // No special-casing of separators: '-' (0x2D) < '.' (0x2E) < '_' (0x5F).
        assert_eq!(prj_file_name_compare("a-b", "a.b"), Ordering::Less);
        assert_eq!(prj_file_name_compare("a.b", "a_b"), Ordering::Less);
        assert_eq!(prj_file_name_compare("a-b", "a_b"), Ordering::Less);
        // '.' (0x2E) < '0' (0x30) < '+' ... check '+' (0x2B) < '-' (0x2D).
        assert_eq!(prj_file_name_compare("a+b", "a-b"), Ordering::Less);
        // Digits come before uppercase letters: '9' (0x39) < 'A' (0x41).
        assert_eq!(prj_file_name_compare("9x", "ax"), Ordering::Less);
    }

    #[test]
    fn digits_are_lexicographic_not_numeric() {
        // No natural/numeric sort: "file10" sorts before "file2" because
        // '1' (0x31) < '2' (0x32).
        assert_eq!(prj_file_name_compare("file10", "file2"), Ordering::Less);
        assert_eq!(prj_file_name_compare("file2", "file10"), Ordering::Greater);
        assert_eq!(prj_file_name_compare("v1", "v1"), Ordering::Equal);
    }

    #[test]
    fn prefix_sorts_before_longer() {
        assert_eq!(prj_file_name_compare("abc", "abcd"), Ordering::Less);
        assert_eq!(prj_file_name_compare("abcd", "abc"), Ordering::Greater);
        assert_eq!(prj_file_name_compare("", "a"), Ordering::Less);
        assert_eq!(prj_file_name_compare("", ""), Ordering::Equal);
        // Case-insensitive prefix.
        assert_eq!(prj_file_name_compare("ABC", "abcd"), Ordering::Less);
    }

    #[test]
    fn full_enumeration_sort_differs_from_str_cmp() {
        // A directory listing resembling a conda env, in scrambled order.
        let mut prj_sorted = vec![
            "Scripts",
            "activate",
            "_conda",
            "Library",
            "conda.exe",
            "LICENSE",
            "README.md",
            "bin",
            "etc",
        ];
        let mut str_sorted = prj_sorted.clone();

        prj_sorted.sort_by(|a, b| prj_file_name_compare(a, b));
        str_sorted.sort();

        // PrjFileNameCompare (case-insensitive): '_' (0x5F) is greater than any
        // letter once letters are upcased, so "_conda" sorts last.
        assert_eq!(
            prj_sorted,
            vec![
                "activate",
                "bin",
                "conda.exe",
                "etc",
                "Library",
                "LICENSE",
                "README.md",
                "Scripts",
                "_conda",
            ]
        );

        // Plain str::cmp is byte order: all capitalized names come first, and
        // "_conda" lands among/after them (0x5F > uppercase, < lowercase).
        assert_ne!(prj_sorted, str_sorted);
    }

    #[test]
    fn non_ascii_bmp_case_folding() {
        // Latin-1 accented letters fold case (1:1 within the BMP).
        assert_eq!(prj_file_name_compare("café", "CAFÉ"), Ordering::Equal);
        assert_eq!(prj_file_name_compare("Ünïcödé", "ünïcödé"), Ordering::Equal);
        // 'ß' (U+00DF) has no single-unit uppercase, so it is left unchanged
        // (we do NOT expand to "SS"); it therefore only equals itself.
        assert_eq!(prj_file_name_compare("straße", "straße"), Ordering::Equal);
        assert_ne!(prj_file_name_compare("straße", "STRASSE"), Ordering::Equal);
    }

    /// On a real Windows host, assert our pure-Rust ordering agrees with the
    /// authoritative Win32 `PrjFileNameCompare` for a battery of names. This is
    /// the check that guarantees the enumeration sort matches what ProjFS
    /// expects; it cannot run on non-Windows CI.
    #[cfg(windows)]
    #[test]
    fn agrees_with_win32_prj_file_name_compare() {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::ProjectedFileSystem::PrjFileNameCompare;
        use windows::core::PCWSTR;

        fn wide(s: &str) -> Vec<u16> {
            std::ffi::OsStr::new(s)
                .encode_wide()
                .chain(Some(0))
                .collect()
        }
        fn win32(a: &str, b: &str) -> Ordering {
            let aw = wide(a);
            let bw = wide(b);
            let r = unsafe { PrjFileNameCompare(PCWSTR(aw.as_ptr()), PCWSTR(bw.as_ptr())) };
            r.cmp(&0)
        }

        let names = [
            "activate",
            "Activate",
            "ACTIVATE",
            "_conda",
            "conda.exe",
            "Library",
            "LICENSE",
            "README.md",
            "Scripts",
            "bin",
            "etc",
            "a-b",
            "a.b",
            "a_b",
            "a+b",
            "file2",
            "file10",
            "café",
            "CAFÉ",
            "straße",
            "python3.11",
            "Python3.11",
            "",
        ];
        for &a in &names {
            for &b in &names {
                assert_eq!(
                    prj_file_name_compare(a, b),
                    win32(a, b),
                    "divergence for {a:?} vs {b:?}"
                );
            }
        }
    }
}
