//! This crate provides helper functions to activate and deactivate virtual environments.
#![deny(missing_docs)]

pub mod activation;
pub mod run;
pub mod sandbox;
pub mod shell;
pub use run::run_in_environment;

// Initialize the sandbox when the crate is loaded
#[cfg(test)]
#[ctor::ctor]
fn init_sandbox() {
    rattler_sandbox::init_sandbox();
}
