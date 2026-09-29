use std::io::Write;

use miette::{Context, IntoDiagnostic};

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
pub mod hyperlink;
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
pub mod skill;
pub mod solve;
pub mod table;
#[cfg(feature = "sigstore")]
pub mod verify_attestation;
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

/// Turns a broken pipe into a successful `false` instead of an error.
///
/// Output of the cli is often piped into another program (e.g. `head` or
/// `file -`) that may exit before it read everything we have to write. A closed
/// stdout is a normal way for such a pipeline to end, so callers that have more
/// data to write should stop quietly once this returns `false`.
pub fn ignore_broken_pipe(result: std::io::Result<()>) -> miette::Result<bool> {
    match result {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => Ok(false),
        Err(err) => Err(err).into_diagnostic(),
    }
}

/// Writes all of `bytes` to stdout and flushes it.
///
/// A closed stdout is not reported as an error, see [`ignore_broken_pipe`].
pub fn write_all_to_stdout(bytes: &[u8]) -> miette::Result<()> {
    let mut stdout = std::io::stdout();
    if ignore_broken_pipe(stdout.write_all(bytes)).context("failed to write to stdout")? {
        ignore_broken_pipe(stdout.flush()).context("failed to flush stdout")?;
    }
    Ok(())
}

/// Writes `urls` to stdout, one per line.
///
/// This output is meant to be piped (e.g. into `head`), so a closed stdout is
/// a normal way to end instead of an error, and the lines are never decorated
/// with [`hyperlink`]s.
pub fn print_url_lines(
    urls: impl IntoIterator<Item = impl std::fmt::Display>,
) -> miette::Result<()> {
    let mut stdout = std::io::stdout().lock();
    for url in urls {
        if !ignore_broken_pipe(writeln!(stdout, "{url}"))? {
            return Ok(());
        }
    }
    ignore_broken_pipe(stdout.flush())?;
    Ok(())
}
