#![deny(missing_docs, dead_code)]

//! This crate provides a set of libraries for working with Python packages installed
//! witin conda environments. This crate is a subset of libraries were originally published
//! in the `rattler_installs_packages` crate.
//! 
//! TODO: Go through the public modules listed here and think about providing usage examples
//!       of the structs and such listed there.
//! 

// Private modules
mod utils;

// Public modules
pub mod artifacts;
pub mod index;
pub mod python_env;
pub mod types;
