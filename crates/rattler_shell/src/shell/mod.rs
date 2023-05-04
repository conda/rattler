//! This module contains the [`Shell`] trait and implementations for various shells.

use enum_dispatch::enum_dispatch;
use itertools::Itertools;
use std::process::Command;
use std::{
    fmt::Write,
    path::{Path, PathBuf},
};

/// A trait for generating shell scripts.
/// The trait is implemented for each shell individually.
///
/// # Example
///
/// ```
/// use std::path::PathBuf;
/// use rattler_shell::shell::Bash;
/// use rattler_shell::shell::Shell;
///
/// let mut script = String::new();
/// let shell = Bash;
/// shell.set_env_var(&mut script, "FOO", "bar").unwrap();
///
/// assert_eq!(script, "export FOO=\"bar\"\n");
/// ```
#[enum_dispatch(ShellEnum)]
pub trait Shell {
    /// Set an env var by `export`-ing it.
    fn set_env_var(&self, f: &mut impl Write, env_var: &str, value: &str) -> std::fmt::Result;

    /// Unset an env var by `unset`-ing it.
    fn unset_env_var(&self, f: &mut impl Write, env_var: &str) -> std::fmt::Result;

    /// Run a script in the current shell.
    fn run_script(&self, f: &mut impl Write, path: &Path) -> std::fmt::Result;

    /// Executes a command in the current shell. Use [`Self::run_script`] when you want to run
    /// another shell script.
    fn run_command<'a>(
        &self,
        f: &mut impl Write,
        command: impl IntoIterator<Item = &'a str> + 'a,
    ) -> std::fmt::Result {
        write!(f, "{}", command.into_iter().join(" "))
    }

    /// Set the PATH variable to the given paths.
    fn set_path(&self, f: &mut impl Write, paths: &[PathBuf]) -> std::fmt::Result {
        let path = std::env::join_paths(paths).unwrap();
        self.set_env_var(f, "PATH", path.to_str().unwrap())
    }

    /// The extension that shell scripts for this interpreter usually use.
    fn extension(&self) -> &str;

    /// Constructs a [`Command`] that will execute the specified script by this shell.
    fn create_run_script_command(&self, path: &Path) -> Command;
}

/// Convert a native PATH on Windows to a Unix style path usign cygpath.
fn native_path_to_unix(path: &str) -> Result<String, std::io::Error> {
    // call cygpath on Windows to convert paths to Unix style
    let output = std::process::Command::new("cygpath")
        .arg("--unix")
        .arg("--path")
        .arg(path)
        .output()
        .unwrap();

    if output.status.success() {
        return Ok(String::from_utf8(output.stdout)
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Failed to convert path to Unix style",
                )
            })?
            .trim()
            .to_string());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        "Failed to convert path to Unix style",
    ))
}

/// A [`Shell`] implementation for the Bash shell.
#[derive(Debug, Clone, Copy)]
pub struct Bash;

impl Shell for Bash {
    fn set_env_var(&self, f: &mut impl Write, env_var: &str, value: &str) -> std::fmt::Result {
        writeln!(f, "export {}=\"{}\"", env_var, value)
    }

    fn unset_env_var(&self, f: &mut impl Write, env_var: &str) -> std::fmt::Result {
        writeln!(f, "unset {}", env_var)
    }

    fn run_script(&self, f: &mut impl Write, path: &Path) -> std::fmt::Result {
        writeln!(f, ". \"{}\"", path.to_string_lossy())
    }

    fn extension(&self) -> &str {
        "sh"
    }

    fn set_path(&self, f: &mut impl Write, paths: &[PathBuf]) -> std::fmt::Result {
        let path = std::env::join_paths(paths).unwrap();

        // check if we are on Windows, and if yes, convert native path to unix for (Git) Bash
        if cfg!(windows) {
            let path = native_path_to_unix(path.to_str().unwrap()).unwrap();
            return self.set_env_var(f, "PATH", &path);
        }

        self.set_env_var(f, "PATH", path.to_str().unwrap())
    }

    fn create_run_script_command(&self, path: &Path) -> Command {
        let mut cmd = Command::new("bash");

        // check if we are on Windows, and if yes, convert native path to unix for (Git) Bash
        if cfg!(windows) {
            cmd.arg(native_path_to_unix(path.to_str().unwrap()).unwrap());
        } else {
            cmd.arg(path);
        }

        cmd
    }
}

/// A [`Shell`] implementation for the Zsh shell.
#[derive(Debug, Clone, Copy)]
pub struct Zsh;

impl Shell for Zsh {
    fn set_env_var(&self, f: &mut impl Write, env_var: &str, value: &str) -> std::fmt::Result {
        writeln!(f, "export {}=\"{}\"", env_var, value)
    }

    fn unset_env_var(&self, f: &mut impl Write, env_var: &str) -> std::fmt::Result {
        writeln!(f, "unset {}", env_var)
    }

    fn run_script(&self, f: &mut impl Write, path: &Path) -> std::fmt::Result {
        writeln!(f, ". \"{}\"", path.to_string_lossy())
    }

    fn extension(&self) -> &str {
        "zsh"
    }

    fn create_run_script_command(&self, path: &Path) -> Command {
        let mut cmd = Command::new("zsh");
        cmd.arg(path);
        cmd
    }
}

/// A [`Shell`] implementation for the Xonsh shell.
#[derive(Debug, Clone, Copy)]
pub struct Xonsh;

impl Shell for Xonsh {
    fn set_env_var(&self, f: &mut impl Write, env_var: &str, value: &str) -> std::fmt::Result {
        writeln!(f, "${} = \"{}\"", env_var, value)
    }

    fn unset_env_var(&self, f: &mut impl Write, env_var: &str) -> std::fmt::Result {
        writeln!(f, "del ${}", env_var)
    }

    fn run_script(&self, f: &mut impl Write, path: &Path) -> std::fmt::Result {
        writeln!(f, "source-bash \"{}\"", path.to_string_lossy())
    }

    fn extension(&self) -> &str {
        "sh"
    }

    fn create_run_script_command(&self, path: &Path) -> Command {
        let mut cmd = Command::new("xonsh");
        cmd.arg(path);
        cmd
    }
}

/// A [`Shell`] implementation for the cmd.exe shell.
#[derive(Debug, Clone, Copy)]
pub struct CmdExe;

impl Shell for CmdExe {
    fn set_env_var(&self, f: &mut impl Write, env_var: &str, value: &str) -> std::fmt::Result {
        writeln!(f, "@SET \"{}={}\"", env_var, value)
    }

    fn unset_env_var(&self, f: &mut impl Write, env_var: &str) -> std::fmt::Result {
        writeln!(f, "@SET {}=", env_var)
    }

    fn run_script(&self, f: &mut impl Write, path: &Path) -> std::fmt::Result {
        writeln!(f, "@CALL \"{}\"", path.to_string_lossy())
    }

    fn run_command<'a>(
        &self,
        f: &mut impl Write,
        command: impl IntoIterator<Item = &'a str> + 'a,
    ) -> std::fmt::Result {
        write!(f, "@{}", command.into_iter().join(" "))
    }

    fn extension(&self) -> &str {
        "bat"
    }

    fn create_run_script_command(&self, path: &Path) -> Command {
        let mut cmd = Command::new("cmd.exe");
        cmd.arg("/D").arg("/C").arg(path);
        cmd
    }
}

/// A [`Shell`] implementation for PowerShell.
#[derive(Debug, Clone, Copy)]
pub struct PowerShell;

impl Shell for PowerShell {
    fn set_env_var(&self, f: &mut impl Write, env_var: &str, value: &str) -> std::fmt::Result {
        writeln!(f, "$Env:{} = \"{}\"", env_var, value)
    }

    fn unset_env_var(&self, f: &mut impl Write, env_var: &str) -> std::fmt::Result {
        writeln!(f, "$Env:{}=\"\"", env_var)
    }

    fn run_script(&self, f: &mut impl Write, path: &Path) -> std::fmt::Result {
        writeln!(f, ". \"{}\"", path.to_string_lossy())
    }

    fn extension(&self) -> &str {
        "ps1"
    }

    fn create_run_script_command(&self, path: &Path) -> Command {
        let mut cmd = Command::new("powershell");
        cmd.arg(path);
        cmd
    }
}

/// A [`Shell`] implementation for the Fish shell.
#[derive(Debug, Clone, Copy)]
pub struct Fish;

impl Shell for Fish {
    fn set_env_var(&self, f: &mut impl Write, env_var: &str, value: &str) -> std::fmt::Result {
        writeln!(f, "set -gx {} \"{}\"", env_var, value)
    }

    fn unset_env_var(&self, f: &mut impl Write, env_var: &str) -> std::fmt::Result {
        writeln!(f, "set -e {}", env_var)
    }

    fn run_script(&self, f: &mut impl Write, path: &Path) -> std::fmt::Result {
        writeln!(f, "source \"{}\"", path.to_string_lossy())
    }

    fn extension(&self) -> &str {
        "fish"
    }

    fn create_run_script_command(&self, path: &Path) -> Command {
        let mut cmd = Command::new("fish");
        cmd.arg(path);
        cmd
    }
}

/// A generic [`Shell`] implementation for concrete shell types.
#[enum_dispatch]
#[allow(missing_docs)]
#[derive(Clone)]
pub enum ShellEnum {
    Bash,
    Zsh,
    Xonsh,
    CmdExe,
    PowerShell,
    Fish,
}

impl ShellEnum {
    /// Returns the shell for the current system.
    pub fn detect_from_environment() -> Option<Self> {
        // TODO: Make this a bit better
        if cfg!(windows) {
            Some(CmdExe.into())
        } else {
            Some(Bash.into())
        }
    }
}

/// A helper struct for generating shell scripts.
pub struct ShellScript<T: Shell> {
    /// The shell class to generate the script for.
    shell: T,
    /// The contents of the script.
    pub contents: String,
}

impl<T: Shell> ShellScript<T> {
    /// Create a new [`ShellScript`] for the given shell.
    pub fn new(shell: T) -> Self {
        Self {
            shell,
            contents: String::new(),
        }
    }

    /// Export an environment variable.
    pub fn set_env_var(&mut self, env_var: &str, value: &str) -> &mut Self {
        self.shell
            .set_env_var(&mut self.contents, env_var, value)
            .unwrap();
        self
    }

    /// Unset an environment variable.
    pub fn unset_env_var(&mut self, env_var: &str) -> &mut Self {
        self.shell
            .unset_env_var(&mut self.contents, env_var)
            .unwrap();
        self
    }

    /// Set the PATH environment variable to the given paths.
    pub fn set_path(&mut self, paths: &[PathBuf]) -> &mut Self {
        self.shell.set_path(&mut self.contents, paths).unwrap();
        self
    }

    /// Run a script in the generated shell script.
    pub fn run_script(&mut self, path: &Path) -> &mut Self {
        self.shell.run_script(&mut self.contents, path).unwrap();
        self
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_bash() {
        let mut script = ShellScript::new(Bash);

        script
            .set_env_var("FOO", "bar")
            .unset_env_var("FOO")
            .run_script(&PathBuf::from_str("foo.sh").expect("blah"));

        insta::assert_snapshot!(script.contents);
    }
}
