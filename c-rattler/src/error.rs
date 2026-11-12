use std::cell::RefCell;

use crate::string::RattlerStr;

/// Status code returned by fallible functions.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RattlerStatus {
    /// The operation succeeded.
    Ok = 0,
    /// An argument was invalid, e.g. a `NULL` pointer or a string that is not
    /// valid UTF-8.
    InvalidArgument = 1,
    /// The input could not be parsed.
    ParseError = 2,
    /// An unexpected internal error (a Rust panic) occurred.
    Panic = 3,
}

/// An error that is reported to the caller as a status code and message.
#[derive(Debug)]
pub(crate) struct Error {
    pub status: RattlerStatus,
    pub message: String,
}

impl Error {
    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self {
            status: RattlerStatus::InvalidArgument,
            message: message.into(),
        }
    }

    pub fn parse(err: impl std::fmt::Display) -> Self {
        Self {
            status: RattlerStatus::ParseError,
            message: err.to_string(),
        }
    }
}

thread_local! {
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub(crate) fn set_last_error(message: String) {
    LAST_ERROR.with(|e| *e.borrow_mut() = Some(message));
}

/// Takes the message of the last error that occurred on the calling thread.
///
/// Returns a string with a `NULL` `data` pointer if no error occurred since
/// the last call. The returned string must be released with
/// `rattler_str_free`.
#[unsafe(no_mangle)]
pub extern "C" fn rattler_take_last_error() -> RattlerStr {
    RattlerStr::from_option(LAST_ERROR.with(|e| e.borrow_mut().take()))
}
