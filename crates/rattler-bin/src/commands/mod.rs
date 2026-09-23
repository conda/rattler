use std::io::Write;

use miette::IntoDiagnostic;

pub mod auth;
pub mod client;
pub mod compare_packages;
pub mod completion;
pub mod create;
pub mod download;
pub mod exec;
pub mod extract;
pub mod fetch_file;
pub mod gateway;
pub mod info;
pub mod inspect;
pub mod link;
pub mod list;
pub mod menu;
pub mod package_source;
pub mod prefix;
pub mod progress;
pub mod run;
pub mod search;
pub mod shell_hook;
pub mod solve;
pub mod table;
pub mod virtual_packages;
pub mod whoneeds;

/// Machine-readable output formats for package queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum QueryOutputFormat {
    /// Output all matching records as JSON.
    Json,
    /// Output package URLs, one per line.
    Urls,
}

/// Writes `urls` to stdout, one per line.
///
/// This output is meant to be piped (e.g. into `head`), so a closed stdout is
/// a normal way to end instead of an error.
pub fn print_url_lines(
    urls: impl IntoIterator<Item = impl std::fmt::Display>,
) -> miette::Result<()> {
    let mut stdout = std::io::stdout().lock();
    for url in urls {
        if let Err(err) = writeln!(stdout, "{url}") {
            if err.kind() == std::io::ErrorKind::BrokenPipe {
                return Ok(());
            }
            return Err(err).into_diagnostic();
        }
    }
    if let Err(err) = stdout.flush()
        && err.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(err).into_diagnostic();
    }
    Ok(())
}
