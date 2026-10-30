use std::{ffi::c_char, str::FromStr};

use rattler_conda_types::{MatchSpec, Matches, PackageName, PackageRecord, package::BuildString};

use crate::{
    RattlerParseStrictness, RattlerStatus,
    error::Error,
    ffi::{as_ref, free, guard, guard_infallible, into_raw, to_str, write_out},
    string::RattlerStr,
    version::RattlerVersion,
};

/// A conda match spec, e.g. `numpy >=1.20,<2 *_cpython` or
/// `conda-forge::python 3.12.*`.
///
/// Created with `rattler_match_spec_parse` and released with
/// `rattler_match_spec_free`.
pub struct RattlerMatchSpec(MatchSpec);

/// Returns a reference to the match spec behind `ptr`, aborting if it is
/// `NULL`.
///
/// # Safety
///
/// `ptr` must be `NULL` or point to a valid `RattlerMatchSpec`.
unsafe fn match_spec<'a>(ptr: *const RattlerMatchSpec) -> &'a MatchSpec {
    // SAFETY: guaranteed by the caller.
    &unsafe { ptr.as_ref() }
        .expect("RattlerMatchSpec pointer must not be NULL")
        .0
}

/// Parses a match spec from a UTF-8 string of `len` bytes.
///
/// On success `*out` is set to a newly allocated match spec that must be
/// released with `rattler_match_spec_free`.
///
/// # Safety
///
/// `str` must point to `len` readable bytes and `out` must be valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_match_spec_parse(
    str: *const c_char,
    len: usize,
    strictness: RattlerParseStrictness,
    out: *mut *mut RattlerMatchSpec,
) -> RattlerStatus {
    guard(|| {
        let s = unsafe { to_str(str, len) }?;
        let spec = MatchSpec::from_str(s, rattler_conda_types::ParseStrictness::from(strictness))
            .map_err(Error::parse)?;
        unsafe { write_out(out, into_raw(RattlerMatchSpec(spec))) }
    })
}

/// Releases a match spec. Passing `NULL` is a no-op.
///
/// # Safety
///
/// `spec` must be `NULL` or a match spec that was not freed before.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_match_spec_free(spec: *mut RattlerMatchSpec) {
    unsafe { free(spec) }
}

/// Returns a copy of `spec` that must be released with
/// `rattler_match_spec_free`.
///
/// # Safety
///
/// `spec` must point to a valid match spec.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_match_spec_clone(
    spec: *const RattlerMatchSpec,
) -> *mut RattlerMatchSpec {
    guard_infallible(|| into_raw(RattlerMatchSpec(unsafe { match_spec(spec) }.clone())))
}

/// Returns the string representation of `spec`. The returned string must be
/// released with `rattler_str_free`.
///
/// # Safety
///
/// `spec` must point to a valid match spec.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_match_spec_to_string(spec: *const RattlerMatchSpec) -> RattlerStr {
    guard_infallible(|| RattlerStr::new(unsafe { match_spec(spec) }.to_string()))
}

/// Returns the package name matcher of `spec`, e.g. `numpy` or `jupyter-*`.
/// The returned string must be released with `rattler_str_free`.
///
/// # Safety
///
/// `spec` must point to a valid match spec.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_match_spec_name(spec: *const RattlerMatchSpec) -> RattlerStr {
    guard_infallible(|| RattlerStr::new(unsafe { match_spec(spec) }.name.to_string()))
}

/// Returns the version constraint of `spec`, e.g. `>=1.20,<2`, or a string
/// with a `NULL` `data` pointer if the spec has no version constraint. The
/// returned string must be released with `rattler_str_free`.
///
/// # Safety
///
/// `spec` must point to a valid match spec.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_match_spec_version(spec: *const RattlerMatchSpec) -> RattlerStr {
    guard_infallible(|| {
        RattlerStr::from_option(
            unsafe { match_spec(spec) }
                .version
                .as_ref()
                .map(ToString::to_string),
        )
    })
}

/// Returns the build string matcher of `spec`, e.g. `py312*`, or a string
/// with a `NULL` `data` pointer if the spec has no build constraint. The
/// returned string must be released with `rattler_str_free`.
///
/// # Safety
///
/// `spec` must point to a valid match spec.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_match_spec_build(spec: *const RattlerMatchSpec) -> RattlerStr {
    guard_infallible(|| {
        RattlerStr::from_option(
            unsafe { match_spec(spec) }
                .build
                .as_ref()
                .map(ToString::to_string),
        )
    })
}

/// Returns the build number constraint of `spec`, e.g. `>=2`, or a string
/// with a `NULL` `data` pointer if the spec has no build number constraint.
/// The returned string must be released with `rattler_str_free`.
///
/// # Safety
///
/// `spec` must point to a valid match spec.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_match_spec_build_number(
    spec: *const RattlerMatchSpec,
) -> RattlerStr {
    guard_infallible(|| {
        RattlerStr::from_option(
            unsafe { match_spec(spec) }
                .build_number
                .as_ref()
                .map(ToString::to_string),
        )
    })
}

/// Returns the name of the channel of `spec`, e.g. `conda-forge`, or its base
/// URL if the channel has no name. Returns a string with a `NULL` `data`
/// pointer if the spec does not specify a channel. The returned string must be
/// released with `rattler_str_free`.
///
/// # Safety
///
/// `spec` must point to a valid match spec.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_match_spec_channel(spec: *const RattlerMatchSpec) -> RattlerStr {
    guard_infallible(|| {
        RattlerStr::from_option(
            unsafe { match_spec(spec) }
                .channel
                .as_ref()
                .map(|c| c.name().to_string()),
        )
    })
}

/// Returns the subdir of `spec`, e.g. `linux-64`, or a string with a `NULL`
/// `data` pointer if the spec does not specify a subdir. The returned string
/// must be released with `rattler_str_free`.
///
/// # Safety
///
/// `spec` must point to a valid match spec.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_match_spec_subdir(spec: *const RattlerMatchSpec) -> RattlerStr {
    guard_infallible(|| RattlerStr::from_option(unsafe { match_spec(spec) }.subdir.clone()))
}

/// Returns `true` if `version` satisfies the version constraint of `spec`.
/// A spec without a version constraint matches every version. All other
/// fields of the spec (e.g. the name) are ignored.
///
/// # Safety
///
/// `spec` must point to a valid match spec and `version` to a valid version.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_match_spec_matches_version(
    spec: *const RattlerMatchSpec,
    version: *const RattlerVersion,
) -> bool {
    guard_infallible(|| {
        let spec = unsafe { match_spec(spec) };
        let version = &unsafe { version.as_ref() }
            .expect("RattlerVersion pointer must not be NULL")
            .0;
        spec.version.as_ref().is_none_or(|v| v.matches(version))
    })
}

/// Checks whether a package with the given name, version, build string and
/// build number matches `spec`.
///
/// The channel and subdir of the spec are ignored. A spec that constrains
/// other package metadata that is not passed to this function (e.g. `md5`,
/// `sha256`, `license` or `track_features`) never matches.
///
/// On success `*out` is set to the result.
///
/// # Safety
///
/// `spec` must point to a valid match spec, `name` to `name_len` readable
/// bytes, `version` to a valid version, `build` to `build_len` readable bytes
/// and `out` must be valid for writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_match_spec_matches_package(
    spec: *const RattlerMatchSpec,
    name: *const c_char,
    name_len: usize,
    version: *const RattlerVersion,
    build: *const c_char,
    build_len: usize,
    build_number: u64,
    out: *mut bool,
) -> RattlerStatus {
    guard(|| {
        let spec = &unsafe { as_ref(spec, "spec") }?.0;
        let name = unsafe { to_str(name, name_len) }?;
        let name = PackageName::from_str(name).map_err(Error::parse)?;
        let version = unsafe { as_ref(version, "version") }?.0.clone();
        let build =
            BuildString::from_str(unsafe { to_str(build, build_len) }?).map_err(Error::parse)?;

        let mut record = PackageRecord::new(name, version, build);
        record.build_number = build_number;

        unsafe { write_out(out, spec.matches(&record)) }
    })
}
