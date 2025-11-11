pub mod libsolv_bindings;
mod test_files;

pub use test_files::{
    download_and_cache_file, download_and_cache_file_async, fetch_test_conda_forge_repodata,
    fetch_test_conda_forge_repodata_async, test_data_dir,
};

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// An enum to direct how to update a generated file.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Overwrite the file on disk if it changed.
    Overwrite,

    /// Verify that the file on disk contains the same content as the generated content. Returns an
    /// error if thats not the case.
    Verify,
}

/// A helper to update file on disk if it has changed or verify the contents of the file on disk
/// depending on the `mode`.
fn update(path: &Path, contents: &str, mode: Mode) -> anyhow::Result<()> {
    let old_contents = fs::read_to_string(path)?;
    let old_contents = old_contents.replace("\r\n", "\n");
    let contents = contents.replace("\r\n", "\n");
    if old_contents == contents {
        return Ok(());
    }

    if mode == Mode::Verify {
        let changes = difference::Changeset::new(&old_contents, &contents, "\n");
        anyhow::bail!("==================================================\n`{}` is not up-to-date\n==================================================\n{}", path.display(), changes,);
    }
    eprintln!("updating {}", path.display());
    fs::write(path, contents)?;
    Ok(())
}

/// Reformats the given input with `rustfmt`.
fn reformat(text: impl std::fmt::Display) -> anyhow::Result<String> {
    let mut rustfmt = Command::new("rustfmt")
        //.arg("--config-path")
        //.arg(project_root().join("rustfmt.toml"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    write!(rustfmt.stdin.take().unwrap(), "{text}")?;
    let output = rustfmt.wait_with_output()?;
    let stdout = String::from_utf8(output.stdout)?;
    let preamble = "Generated file, do not edit by hand, see `crate/tools/src`";
    Ok(format!("//! {preamble}\n\n{stdout}"))
}

/// Returns the path to the Cargo manifest directory (or the root of the workspace).
pub fn project_root() -> PathBuf {
    Path::new(&env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

#[cfg(all(test, not(target_env = "musl")))]
mod test {
    use crate::Mode;

    #[test]
    fn libsolv_bindings_up_to_date() {
        if let Err(error) = super::libsolv_bindings::generate(Mode::Verify) {
            panic!("{error}\n\nPlease update the bindings by running\n\n\tcargo run --bin tools -- gen-libsolv-bindings\n\nMake sure you run that command both on Windows and on a unix machine!\n");
        }
    }
}
