//! C bindings to rattler.
//!
//! # Conventions
//!
//! * All rattler objects are exposed as opaque pointers, e.g. `RattlerVersion*`.
//!   Every object returned to the caller must be released with the matching
//!   `*_free` function. Passing `NULL` to a `*_free` function is a no-op.
//! * Strings are passed in as a pointer and a length in bytes and do not need
//!   to be NUL-terminated. They must be valid UTF-8.
//! * Strings returned to the caller are [`RattlerStr`] values that must be
//!   released with `rattler_str_free`.
//! * Fallible functions return a [`RattlerStatus`] and write their result to an
//!   out-parameter. On failure a human-readable error message can be retrieved
//!   with `rattler_take_last_error`.
//! * Panics never cross the FFI boundary, they are reported as
//!   [`RattlerStatus::Panic`].

mod error;
mod ffi;
mod match_spec;
mod string;
mod version;

pub use error::{RattlerStatus, rattler_take_last_error};
pub use match_spec::*;
pub use string::{RattlerStr, rattler_str_free};
pub use version::*;

/// The strictness that is used when parsing versions and match specs.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RattlerParseStrictness {
    /// Allow common mistakes and try to recover from them.
    Lenient = 0,
    /// Only allow input that strictly adheres to the specification.
    Strict = 1,
}

impl From<RattlerParseStrictness> for rattler_conda_types::ParseStrictness {
    fn from(value: RattlerParseStrictness) -> Self {
        match value {
            RattlerParseStrictness::Lenient => Self::Lenient,
            RattlerParseStrictness::Strict => Self::Strict,
        }
    }
}

/// Returns the version of the rattler C library as a static, NUL-terminated
/// string. The returned pointer must not be freed.
#[unsafe(no_mangle)]
pub extern "C" fn rattler_library_version() -> *const std::ffi::c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}
