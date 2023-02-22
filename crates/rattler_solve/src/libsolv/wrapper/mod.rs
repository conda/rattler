//! This module provides a safe wrapper around libsolv

use std::ffi::CString;

mod ffi;
pub mod flags;
pub mod keys;
pub mod pool;
pub mod queue;
pub mod repo;
pub mod repodata;
pub mod solvable;
pub mod solver;
pub mod transaction;

/// Convenience method that converts a string reference to a CString
fn c_string<T: AsRef<str>>(str: T) -> CString {
    CString::new(str.as_ref()).expect("should be convertable from string")
}
