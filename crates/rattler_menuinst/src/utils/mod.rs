pub mod slugify;
pub use slugify::slugify;
pub mod terminal;
pub use terminal::log_output;

#[cfg(target_family = "unix")]
pub use terminal::run_pre_create_command;
