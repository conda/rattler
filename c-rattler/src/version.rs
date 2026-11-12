use std::{cmp::Ordering, ffi::c_char, str::FromStr};

use rattler_conda_types::Version;

use crate::{
    RattlerStatus,
    error::Error,
    ffi::{free, guard, guard_infallible, into_raw, to_str, write_out},
    string::RattlerStr,
};

/// A conda package version, e.g. `1.2.3`, `1!2.0a1` or `3.1.0+local`.
///
/// Created with `rattler_version_parse` and released with
/// `rattler_version_free`.
pub struct RattlerVersion(pub(crate) Version);

/// Returns a reference to the version behind `ptr`, aborting if it is `NULL`.
///
/// # Safety
///
/// `ptr` must be `NULL` or point to a valid `RattlerVersion`.
unsafe fn version<'a>(ptr: *const RattlerVersion) -> &'a Version {
    // SAFETY: guaranteed by the caller.
    &unsafe { ptr.as_ref() }
        .expect("RattlerVersion pointer must not be NULL")
        .0
}

/// Parses a version from a UTF-8 string of `len` bytes.
///
/// On success `*out` is set to a newly allocated version that must be
/// released with `rattler_version_free`.
///
/// # Safety
///
/// `str` must point to `len` readable bytes and `out` must be valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_version_parse(
    str: *const c_char,
    len: usize,
    out: *mut *mut RattlerVersion,
) -> RattlerStatus {
    guard(|| {
        let s = unsafe { to_str(str, len) }?;
        let version = Version::from_str(s).map_err(Error::parse)?;
        unsafe { write_out(out, into_raw(RattlerVersion(version))) }
    })
}

/// Releases a version. Passing `NULL` is a no-op.
///
/// # Safety
///
/// `version` must be `NULL` or a version that was not freed before.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_version_free(version: *mut RattlerVersion) {
    unsafe { free(version) }
}

/// Returns a copy of `version` that must be released with
/// `rattler_version_free`.
///
/// # Safety
///
/// `version` must point to a valid version.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_version_clone(
    version: *const RattlerVersion,
) -> *mut RattlerVersion {
    guard_infallible(|| into_raw(RattlerVersion(unsafe { self::version(version) }.clone())))
}

/// Returns the string representation of `version`. The returned string must
/// be released with `rattler_str_free`.
///
/// # Safety
///
/// `version` must point to a valid version.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_version_to_string(version: *const RattlerVersion) -> RattlerStr {
    guard_infallible(|| RattlerStr::new(unsafe { self::version(version) }.to_string()))
}

/// Compares two versions. Returns a negative number if `a < b`, zero if
/// `a == b` and a positive number if `a > b`.
///
/// # Safety
///
/// `a` and `b` must point to valid versions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_version_compare(
    a: *const RattlerVersion,
    b: *const RattlerVersion,
) -> i32 {
    guard_infallible(|| match unsafe { version(a).cmp(version(b)) } {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    })
}

/// Returns the epoch of `version`, or `0` if it does not have an explicit
/// epoch.
///
/// # Safety
///
/// `version` must point to a valid version.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_version_epoch(version: *const RattlerVersion) -> u64 {
    guard_infallible(|| unsafe { self::version(version) }.epoch())
}

/// Returns `true` if `version` has an explicit epoch, e.g. `1!2.0`.
///
/// # Safety
///
/// `version` must point to a valid version.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_version_has_epoch(version: *const RattlerVersion) -> bool {
    guard_infallible(|| unsafe { self::version(version) }.has_epoch())
}

/// Returns `true` if `version` has a local version part, e.g. `1.0+local`.
///
/// # Safety
///
/// `version` must point to a valid version.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_version_has_local(version: *const RattlerVersion) -> bool {
    guard_infallible(|| unsafe { self::version(version) }.has_local())
}

/// Returns `true` if `version` is a development version, e.g. `1.0.dev1`.
///
/// # Safety
///
/// `version` must point to a valid version.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_version_is_dev(version: *const RattlerVersion) -> bool {
    guard_infallible(|| unsafe { self::version(version) }.is_dev())
}

/// Returns the number of segments in `version`, e.g. `3` for `1.2.3`.
///
/// # Safety
///
/// `version` must point to a valid version.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_version_segment_count(version: *const RattlerVersion) -> usize {
    guard_infallible(|| unsafe { self::version(version) }.segment_count())
}

/// Tries to extract the major and minor component of `version`.
///
/// Returns `true` and writes the components to `major` and `minor` if the
/// first two segments of `version` are numeric. Returns `false` and leaves
/// `major` and `minor` untouched otherwise.
///
/// # Safety
///
/// `version` must point to a valid version, `major` and `minor` must be valid
/// for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_version_as_major_minor(
    version: *const RattlerVersion,
    major: *mut u64,
    minor: *mut u64,
) -> bool {
    guard_infallible(
        || match unsafe { self::version(version) }.as_major_minor() {
            Some((ma, mi)) => {
                assert!(!major.is_null() && !minor.is_null());
                unsafe {
                    major.write(ma);
                    minor.write(mi);
                }
                true
            }
            None => false,
        },
    )
}

/// Returns `true` if `version` starts with all segments of `prefix`, e.g.
/// `1.2.3` starts with `1.2`.
///
/// # Safety
///
/// `version` and `prefix` must point to valid versions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_version_starts_with(
    version: *const RattlerVersion,
    prefix: *const RattlerVersion,
) -> bool {
    guard_infallible(|| unsafe { self::version(version).starts_with(self::version(prefix)) })
}

/// Returns `true` if `version` is compatible with `other` in the sense of the
/// `~=` operator, e.g. `1.2.3` is compatible with `1.2`.
///
/// # Safety
///
/// `version` and `other` must point to valid versions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_version_compatible_with(
    version: *const RattlerVersion,
    other: *const RattlerVersion,
) -> bool {
    guard_infallible(|| unsafe { self::version(version).compatible_with(self::version(other)) })
}
