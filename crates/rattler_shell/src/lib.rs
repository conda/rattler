#![deny(missing_docs)]

//! This crate provides helper functions to activate and deactivate virtual environments.

pub mod activation;
pub mod run;
pub mod shell;
pub use run::run_in_environment;
