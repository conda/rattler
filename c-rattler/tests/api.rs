//! Exercises the C API from Rust through the `rlib`.

use std::ptr;

use rattler::*;

fn take_string(s: RattlerStr) -> Option<String> {
    if s.data.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.data.cast::<u8>(), s.len) };
    let result = String::from_utf8(bytes.to_vec()).unwrap();
    unsafe { rattler_str_free(s) };
    Some(result)
}

fn parse_version(s: &str) -> *mut RattlerVersion {
    let mut out = ptr::null_mut();
    let status = unsafe { rattler_version_parse(s.as_ptr().cast(), s.len(), &mut out) };
    assert_eq!(
        status,
        RattlerStatus::Ok,
        "{:?}",
        take_string(rattler_take_last_error())
    );
    out
}

fn parse_match_spec(s: &str) -> *mut RattlerMatchSpec {
    let mut out = ptr::null_mut();
    let status = unsafe {
        rattler_match_spec_parse(
            s.as_ptr().cast(),
            s.len(),
            RattlerParseStrictness::Strict,
            &mut out,
        )
    };
    assert_eq!(
        status,
        RattlerStatus::Ok,
        "{:?}",
        take_string(rattler_take_last_error())
    );
    out
}

#[test]
fn version_roundtrip_and_accessors() {
    let v = parse_version("1!2.3.4.dev1+local");
    unsafe {
        assert_eq!(
            take_string(rattler_version_to_string(v)).unwrap(),
            "1!2.3.4.dev1+local"
        );
        assert_eq!(rattler_version_epoch(v), 1);
        assert!(rattler_version_has_epoch(v));
        assert!(rattler_version_has_local(v));
        assert!(rattler_version_is_dev(v));
        assert_eq!(rattler_version_segment_count(v), 4);

        let (mut major, mut minor) = (0, 0);
        assert!(rattler_version_as_major_minor(v, &mut major, &mut minor));
        assert_eq!((major, minor), (2, 3));

        let clone = rattler_version_clone(v);
        assert_eq!(rattler_version_compare(v, clone), 0);
        rattler_version_free(clone);
        rattler_version_free(v);
        rattler_version_free(ptr::null_mut());
    }
}

#[test]
fn version_compare() {
    let a = parse_version("1.2.3");
    let b = parse_version("1.10");
    let prefix = parse_version("1.2");
    unsafe {
        assert_eq!(rattler_version_compare(a, b), -1);
        assert_eq!(rattler_version_compare(b, a), 1);
        assert!(rattler_version_starts_with(a, prefix));
        assert!(!rattler_version_starts_with(b, prefix));
        assert!(rattler_version_compatible_with(a, prefix));
        rattler_version_free(a);
        rattler_version_free(b);
        rattler_version_free(prefix);
    }
}

#[test]
fn version_parse_error() {
    let s = "1.2..3";
    let mut out = ptr::null_mut();
    let status = unsafe { rattler_version_parse(s.as_ptr().cast(), s.len(), &mut out) };
    assert_eq!(status, RattlerStatus::ParseError);
    assert!(out.is_null());
    assert!(take_string(rattler_take_last_error()).is_some());
    // The error is consumed by the first call.
    assert!(take_string(rattler_take_last_error()).is_none());
}

#[test]
fn invalid_arguments() {
    let invalid_utf8 = [0xffu8, 0xfe];
    let mut out = ptr::null_mut();
    let status = unsafe { rattler_version_parse(invalid_utf8.as_ptr().cast(), 2, &mut out) };
    assert_eq!(status, RattlerStatus::InvalidArgument);
    assert!(
        take_string(rattler_take_last_error())
            .unwrap()
            .contains("UTF-8")
    );

    let status = unsafe { rattler_version_parse("1.0".as_ptr().cast(), 3, ptr::null_mut()) };
    assert_eq!(status, RattlerStatus::InvalidArgument);
    let _ = rattler_take_last_error();
}

#[test]
fn match_spec_accessors() {
    let spec = parse_match_spec("conda-forge/linux-64::numpy >=1.20,<2 py312*");
    unsafe {
        assert_eq!(take_string(rattler_match_spec_name(spec)).unwrap(), "numpy");
        assert_eq!(
            take_string(rattler_match_spec_version(spec)).unwrap(),
            ">=1.20,<2"
        );
        assert_eq!(
            take_string(rattler_match_spec_build(spec)).unwrap(),
            "py312*"
        );
        assert_eq!(take_string(rattler_match_spec_build_number(spec)), None);
        assert_eq!(
            take_string(rattler_match_spec_channel(spec)).unwrap(),
            "conda-forge"
        );
        assert_eq!(
            take_string(rattler_match_spec_subdir(spec)).unwrap(),
            "linux-64"
        );
        assert!(take_string(rattler_match_spec_to_string(spec)).is_some());

        let clone = rattler_match_spec_clone(spec);
        assert_eq!(
            take_string(rattler_match_spec_name(clone)).unwrap(),
            "numpy"
        );
        rattler_match_spec_free(clone);
        rattler_match_spec_free(spec);
        rattler_match_spec_free(ptr::null_mut());
    }
}

#[test]
fn match_spec_matches() {
    let spec = parse_match_spec("numpy >=1.20,<2 py312*");
    let matching = parse_version("1.26.4");
    let too_new = parse_version("2.0.0");

    let check = |name: &str, version, build: &str| {
        let mut result = false;
        let status = unsafe {
            rattler_match_spec_matches_package(
                spec,
                name.as_ptr().cast(),
                name.len(),
                version,
                build.as_ptr().cast(),
                build.len(),
                0,
                &mut result,
            )
        };
        assert_eq!(status, RattlerStatus::Ok);
        result
    };

    assert!(check("numpy", matching, "py312h1234_0"));
    assert!(!check("numpy", too_new, "py312h1234_0"));
    assert!(!check("numpy", matching, "py311h1234_0"));
    assert!(!check("scipy", matching, "py312h1234_0"));

    unsafe {
        assert!(rattler_match_spec_matches_version(spec, matching));
        assert!(!rattler_match_spec_matches_version(spec, too_new));
        rattler_version_free(matching);
        rattler_version_free(too_new);
        rattler_match_spec_free(spec);
    }
}

#[test]
fn match_spec_strictness() {
    // `==1.0.*` is only accepted in lenient mode.
    let s = "numpy ==1.0.*";
    let mut out = ptr::null_mut();
    let status = unsafe {
        rattler_match_spec_parse(
            s.as_ptr().cast(),
            s.len(),
            RattlerParseStrictness::Strict,
            &mut out,
        )
    };
    assert_eq!(status, RattlerStatus::ParseError);
    let _ = rattler_take_last_error();

    let status = unsafe {
        rattler_match_spec_parse(
            s.as_ptr().cast(),
            s.len(),
            RattlerParseStrictness::Lenient,
            &mut out,
        )
    };
    assert_eq!(status, RattlerStatus::Ok);
    unsafe { rattler_match_spec_free(out) };
}

#[test]
fn library_version() {
    let v = unsafe { std::ffi::CStr::from_ptr(rattler_library_version()) };
    assert_eq!(v.to_str().unwrap(), env!("CARGO_PKG_VERSION"));
}
