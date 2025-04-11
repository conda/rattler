//! This crate provides helper functions to activate and deactivate virtual environments.
#![deny(missing_docs)]

pub mod activation;
pub mod run;
pub mod shell;
pub use run::run_in_environment;
