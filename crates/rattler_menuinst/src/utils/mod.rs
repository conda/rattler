pub mod slugify;
pub use slugify::slugify;
pub mod terminal;
pub use terminal::log_output;

#[cfg(target_family = "unix")]
pub use terminal::run_pre_create_command;

pub fn quote_args<I, S>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .map(|arg| format!(r#""{}""#, arg.as_ref()))
        .collect()
}
