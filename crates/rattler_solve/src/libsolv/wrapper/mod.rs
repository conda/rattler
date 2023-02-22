//! This module provides a mostly safe wrapper around libsolv
//!
//! The design of libsolv makes it difficult to provide a fully-safe wrapper. One of the reasons is
//! that the library is full of doubly-linked data structures (e.g. Pool <-> Repo <-> Solvable),
//! which are the bane of Rust's ownership system.
//!
//! Some types, like [`queue::Queue`] can be modelled using ownership semantics, but we treat most
//! other types as interiorly mutable, meaning that you can perform write operations through shared
//! references (`&`).

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
