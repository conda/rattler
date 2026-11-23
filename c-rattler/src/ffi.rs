//! Helpers shared by all FFI functions.

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::error::{Error, RattlerStatus, set_last_error};

/// Runs `f`, converting errors and panics into a [`RattlerStatus`] and
/// recording the error message for `rattler_take_last_error`.
pub(crate) fn guard(f: impl FnOnce() -> Result<(), Error>) -> RattlerStatus {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => RattlerStatus::Ok,
        Ok(Err(err)) => {
            set_last_error(err.message);
            err.status
        }
        Err(panic) => {
            let message = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            set_last_error(format!("rattler panicked: {message}"));
            RattlerStatus::Panic
        }
    }
}

/// Runs an infallible function, aborting the process if it panics instead of
/// unwinding across the FFI boundary.
pub(crate) fn guard_infallible<T>(f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(_) => std::process::abort(),
    }
}

/// Converts a pointer and length into a `&str`.
///
/// # Safety
///
/// If `ptr` is not `NULL` it must point to `len` readable bytes.
pub(crate) unsafe fn to_str<'a>(
    ptr: *const std::ffi::c_char,
    len: usize,
) -> Result<&'a str, Error> {
    if ptr.is_null() {
        return if len == 0 {
            Ok("")
        } else {
            Err(Error::invalid_argument("string pointer is NULL"))
        };
    }
    // SAFETY: guaranteed by the caller.
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) };
    std::str::from_utf8(bytes)
        .map_err(|e| Error::invalid_argument(format!("string is not valid UTF-8: {e}")))
}

/// Writes `value` to the out-parameter `out`.
///
/// # Safety
///
/// If `out` is not `NULL` it must be valid for writes.
pub(crate) unsafe fn write_out<T>(out: *mut T, value: T) -> Result<(), Error> {
    if out.is_null() {
        return Err(Error::invalid_argument("output pointer is NULL"));
    }
    // SAFETY: guaranteed by the caller.
    unsafe { out.write(value) };
    Ok(())
}

/// Moves `value` to the heap and transfers ownership to the caller.
pub(crate) fn into_raw<T>(value: T) -> *mut T {
    Box::into_raw(Box::new(value))
}

/// Reclaims and drops a value created with [`into_raw`]. `NULL` is a no-op.
///
/// # Safety
///
/// `ptr` must be `NULL` or created by [`into_raw`] and not freed before.
pub(crate) unsafe fn free<T>(ptr: *mut T) {
    if !ptr.is_null() {
        // SAFETY: guaranteed by the caller.
        drop(unsafe { Box::from_raw(ptr) });
    }
}

/// Returns a reference to the value behind `ptr`.
///
/// # Safety
///
/// `ptr` must be `NULL` or point to a valid `T`.
pub(crate) unsafe fn as_ref<'a, T>(ptr: *const T, name: &str) -> Result<&'a T, Error> {
    // SAFETY: guaranteed by the caller.
    unsafe { ptr.as_ref() }.ok_or_else(|| Error::invalid_argument(format!("{name} is NULL")))
}
