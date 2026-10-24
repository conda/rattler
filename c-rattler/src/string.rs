use std::ffi::c_char;

/// A UTF-8 string owned by rattler.
///
/// `data` points to `len` bytes of UTF-8 followed by a NUL terminator, so it
/// can also be used as a C string. A `RattlerStr` with a `NULL` `data` pointer
/// represents the absence of a value.
///
/// Strings returned by rattler must be released with `rattler_str_free`.
#[repr(C)]
#[derive(Debug)]
pub struct RattlerStr {
    /// Pointer to the NUL-terminated UTF-8 data, or `NULL`.
    pub data: *const c_char,
    /// Length of the string in bytes, excluding the NUL terminator.
    pub len: usize,
}

impl RattlerStr {
    /// A string that represents the absence of a value.
    pub(crate) const fn null() -> Self {
        Self {
            data: std::ptr::null(),
            len: 0,
        }
    }

    /// Transfers ownership of `value` to the caller.
    pub(crate) fn new(value: impl Into<String>) -> Self {
        let mut bytes = value.into().into_bytes();
        let len = bytes.len();
        bytes.push(0);
        let data = Box::into_raw(bytes.into_boxed_slice());
        Self {
            data: data.cast::<c_char>().cast_const(),
            len,
        }
    }

    pub(crate) fn from_option(value: Option<impl Into<String>>) -> Self {
        value.map_or_else(Self::null, Self::new)
    }
}

/// Releases a string that was returned by rattler. Freeing a string with a
/// `NULL` `data` pointer is a no-op.
///
/// # Safety
///
/// `s` must have been returned by rattler and must not have been freed before.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rattler_str_free(s: RattlerStr) {
    if s.data.is_null() {
        return;
    }
    let slice = std::ptr::slice_from_raw_parts_mut(s.data.cast::<u8>().cast_mut(), s.len + 1);
    // SAFETY: the pointer was created by `RattlerStr::new` from a boxed slice of
    // `len + 1` bytes.
    drop(unsafe { Box::from_raw(slice) });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let s = RattlerStr::new("hello");
        assert_eq!(s.len, 5);
        let c_str = unsafe { std::ffi::CStr::from_ptr(s.data) };
        assert_eq!(c_str.to_str().unwrap(), "hello");
        unsafe { rattler_str_free(s) };
        unsafe { rattler_str_free(RattlerStr::null()) };
    }
}
