//! This module provides a mostly safe wrapper around libsolv
//!
//! The design of libsolv makes it difficult to provide a fully-safe wrapper. One of the reasons is
//! that the library is full of doubly-linked data structures (e.g. Pool <-> Repo <-> Solvable),
//! which are the bane of Rust's ownership system.
//!
//! Some types, like [`queue::Queue`] can be modelled using ownership semantics, but we treat most
//! other types as interiorly mutable, meaning that you can perform write operations through shared
//! references (`&`). This works well as long as you mutate libsolv data through its FFI. If you
//! need to mutate libsolv data from Rust, e.g. when setting up solvables, you will need to obtain
//! a `&mut` references to the relevant data, taking special care to ensure there is no aliasing at
//! hand.

pub mod flags;
pub mod keys;
pub mod pool;
pub mod queue;
pub mod repo;
pub mod repodata;
pub mod solvable;
pub mod solve_goal;
pub mod solve_problem;
pub mod solver;
pub mod transaction;

pub use rattler_libsolv_c as ffi;
