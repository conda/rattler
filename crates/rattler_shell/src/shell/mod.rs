//! This module contains the [`Shell`] trait and implementations for various
//! shells.

use std::{
    borrow::Cow,
    collections::HashMap,
    ffi::OsStr,
    fmt::Write,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
};

use enum_dispatch::enum_dispatch;
use itertools::Itertools;
use rattler_conda_types::Platform;
use thiserror::Error;

use crate::activation::PathModificationBehavior;

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
    /// Write a command to the script that forces the usage of UTF8-encoding for
    /// the shell script.
    fn force_utf8(&self, _f: &mut impl Write) -> std::fmt::Result {
        Ok(())
    }

    /// Set an env var by `export`-ing it.
    fn set_env_var(&self, f: &mut impl Write, env_var: &str, value: &str) -> std::fmt::Result;

    /// Unset an env var by `unset`-ing it.
    fn unset_env_var(&self, f: &mut impl Write, env_var: &str) -> std::fmt::Result;

    /// Run a script in the current shell.
    fn run_script(&self, f: &mut impl Write, path: &Path) -> std::fmt::Result;

    /// Test to see if the path can be executed by the shell, based on the
    /// extension of the path.
    fn can_run_script(&self, path: &Path) -> bool {
        path.is_file()
            && path
                .extension()
                .and_then(OsStr::to_str)
                .map_or(false, |ext| ext == self.extension())
    }

    /// Executes a command in the current shell. Use [`Self::run_script`] when
    /// you want to run another shell script.
    fn run_command<'a>(
        &self,
        f: &mut impl Write,
        command: impl IntoIterator<Item = &'a str> + 'a,
    ) -> std::fmt::Result {
        writeln!(f, "{}", command.into_iter().join(" "))
    }

    /// Set the PATH variable to the given paths.
    fn set_path(
        &self,
        f: &mut impl Write,
        paths: &[PathBuf],
        modification_behavior: PathModificationBehavior,
        platform: &Platform,
    ) -> std::fmt::Result {
        let mut paths_vec = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect_vec();
        // Replace, Append, or Prepend the path variable to the paths.
        let path_var = self.path_var(platform);
        match modification_behavior {
            PathModificationBehavior::Replace => (),
            PathModificationBehavior::Append => paths_vec.insert(0, self.format_env_var(path_var)),
            PathModificationBehavior::Prepend => paths_vec.push(self.format_env_var(path_var)),
        }
        // Create the shell specific list of paths.
        let paths_string = paths_vec.join(self.path_separator(platform));

        self.set_env_var(f, self.path_var(platform), paths_string.as_str())
    }

    /// The extension that shell scripts for this interpreter usually use.
    fn extension(&self) -> &str;

    /// The executable that can be called to start this shell.
    fn executable(&self) -> &str;

    /// Constructs a [`Command`] that will execute the specified script by this
    /// shell.
    fn create_run_script_command(&self, path: &Path) -> Command;

    /// Path separator
    fn path_separator(&self, platform: &Platform) -> &str {
        if platform.is_unix() {
            ":"
        } else {
            ";"
        }
    }

    /// Returns the name of the PATH variable for the given platform. On
    /// Windows, path variables are case-insensitive but not all shells treat
    /// them case-insensitive.
    fn path_var(&self, platform: &Platform) -> &str {
        if platform.is_windows() {
            "Path"
        } else {
            "PATH"
        }
    }

    /// Format the environment variable for the shell.
    fn format_env_var(&self, var_name: &str) -> String {
        format!("${{{var_name}}}")
    }

    /// Emits echoing certain text to stdout.
    fn echo(&self, f: &mut impl Write, text: &str) -> std::fmt::Result {
        writeln!(f, "echo {}", shlex::try_quote(text).unwrap_or_default())
    }

    /// Emits writing all current environment variables to stdout.
    fn print_env(&self, f: &mut impl Write) -> std::fmt::Result {
        writeln!(f, "/usr/bin/env")
    }

    /// Write the script to the writer and do some post-processing for
    /// line-endings. Only really relevant for cmd.exe scripts.
    fn write_script(&self, f: &mut impl std::io::Write, script: &str) -> std::io::Result<()> {
        f.write_all(script.as_bytes())
    }

    /// Parses environment variables emitted by the `Shell::env` command.
    fn parse_env<'i>(&self, env: &'i str) -> HashMap<&'i str, &'i str> {
        env.lines()
            .filter_map(|line| {
                line.split_once('=')
                    // Trim " as CmdExe could add this to its variables.
                    .map(|(key, value)| (key, value.trim_matches('"')))
            })
            .collect()
    }

    /// Get the line ending for this shell. Only `CmdExe` uses `\r\n`.
    fn line_ending(&self) -> &str {
        "\n"
    }
}

/// Convert a native PATH on Windows to a Unix style path using cygpath.
fn native_path_to_unix(path: &str) -> Result<String, std::io::Error> {
    // call cygpath on Windows to convert paths to Unix style
    let output = Command::new("cygpath")
        .arg("--unix")
        .arg("--path")
        .arg(path)
        .output();

    match output {
        Ok(output) if output.status.success() => Ok(String::from_utf8(output.stdout)
            .map_err(|_err| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "failed to convert path to Unix style",
                )
            })?
            .trim()
            .to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(e),
        Err(e) => Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("failed to convert path to Unix style: {e}"),
        )),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "failed to convert path to Unix style: cygpath failed",
        )),
    }
}

/// A [`Shell`] implementation for the Bash shell.
#[derive(Debug, Clone, Copy, Default)]
pub struct Bash;

impl Shell for Bash {
    fn set_env_var(&self, f: &mut impl Write, env_var: &str, value: &str) -> std::fmt::Result {
        writeln!(f, "export {env_var}=\"{value}\"")
    }

    fn unset_env_var(&self, f: &mut impl Write, env_var: &str) -> std::fmt::Result {
        writeln!(f, "unset {env_var}")
    }

    fn run_script(&self, f: &mut impl Write, path: &Path) -> std::fmt::Result {
        writeln!(f, ". \"{}\"", path.to_string_lossy())
    }

    fn set_path(
        &self,
        f: &mut impl Write,
        paths: &[PathBuf],
        modification_behavior: PathModificationBehavior,
        platform: &Platform,
    ) -> std::fmt::Result {
        // Put paths in a vector of the correct format.
        let mut paths_vec = paths
            .iter()
            .map(|path| {
                // check if we are on Windows, and if yes, convert native path to unix for (Git)
                // Bash
                if cfg!(windows) {
                    match native_path_to_unix(path.to_string_lossy().as_ref()) {
                        Ok(path) => path,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            // This indicates that the cypath executable could not be found. In that
                            // case we just ignore any conversion and use the windows path directly.
                            path.to_string_lossy().to_string()
                        }
                        Err(e) => panic!("{e}"),
                    }
                } else {
                    path.to_string_lossy().into_owned()
                }
            })
            .collect_vec();

        // Replace, Append, or Prepend the path variable to the paths.
        let path_var = self.path_var(platform);
        match modification_behavior {
            PathModificationBehavior::Replace => (),
            PathModificationBehavior::Prepend => paths_vec.push(self.format_env_var(path_var)),
            PathModificationBehavior::Append => paths_vec.insert(0, self.format_env_var(path_var)),
        }
        // Create the shell specific list of paths.
        let paths_string = paths_vec.join(self.path_separator(platform));

        self.set_env_var(f, self.path_var(platform), paths_string.as_str())
    }

    fn extension(&self) -> &str {
        "sh"
    }

    fn executable(&self) -> &str {
        "bash"
    }

    fn create_run_script_command(&self, path: &Path) -> Command {
        let mut cmd = Command::new(self.executable());

        // check if we are on Windows, and if yes, convert native path to unix for (Git)
        // Bash
        if cfg!(windows) {
            cmd.arg(native_path_to_unix(path.to_str().unwrap()).unwrap());
        } else {
            cmd.arg(path);
        }

        cmd
    }
}

/// A [`Shell`] implementation for the Zsh shell.
#[derive(Debug, Clone, Copy, Default)]
pub struct Zsh;

impl Shell for Zsh {
    fn set_env_var(&self, f: &mut impl Write, env_var: &str, value: &str) -> std::fmt::Result {
        writeln!(f, "export {env_var}=\"{value}\"")
    }

    fn unset_env_var(&self, f: &mut impl Write, env_var: &str) -> std::fmt::Result {
        writeln!(f, "unset {env_var}")
    }

    fn run_script(&self, f: &mut impl Write, path: &Path) -> std::fmt::Result {
        writeln!(f, ". \"{}\"", path.to_string_lossy())
    }

    fn extension(&self) -> &str {
        "sh"
    }

    fn executable(&self) -> &str {
        "zsh"
    }

    fn create_run_script_command(&self, path: &Path) -> Command {
        let mut cmd = Command::new(self.executable());
        cmd.arg(path);
        cmd
    }
}

/// A [`Shell`] implementation for the Xonsh shell.
#[derive(Debug, Clone, Copy, Default)]
pub struct Xonsh;

impl Shell for Xonsh {
    fn set_env_var(&self, f: &mut impl Write, env_var: &str, value: &str) -> std::fmt::Result {
        writeln!(f, "${env_var} = \"{value}\"")
    }

    fn unset_env_var(&self, f: &mut impl Write, env_var: &str) -> std::fmt::Result {
        writeln!(f, "del ${env_var}")
    }

    fn run_script(&self, f: &mut impl Write, path: &Path) -> std::fmt::Result {
        let ext = path.extension().and_then(OsStr::to_str);
        let cmd = match ext {
            Some("sh") => "source-bash",
            _ => "source",
        };
        writeln!(f, "{} \"{}\"", cmd, path.to_string_lossy())
    }

    fn can_run_script(&self, path: &Path) -> bool {
        path.is_file()
            && path
                .extension()
                .and_then(OsStr::to_str)
                .map_or(false, |ext| ext == "xsh" || ext == "sh")
    }

    fn extension(&self) -> &str {
        "xsh"
    }

    fn executable(&self) -> &str {
        "xonsh"
    }

    fn create_run_script_command(&self, path: &Path) -> Command {
        let mut cmd = Command::new(self.executable());
        cmd.arg(path);
        cmd
    }
}

/// A [`Shell`] implementation for the cmd.exe shell.
#[derive(Debug, Clone, Copy, Default)]
pub struct CmdExe;

impl Shell for CmdExe {
    fn force_utf8(&self, f: &mut impl Write) -> std::fmt::Result {
        writeln!(f, "@chcp 65001 > nul")
    }

    fn set_env_var(&self, f: &mut impl Write, env_var: &str, value: &str) -> std::fmt::Result {
        writeln!(f, "@SET \"{env_var}={value}\"")
    }

    fn unset_env_var(&self, f: &mut impl Write, env_var: &str) -> std::fmt::Result {
        writeln!(f, "@SET {env_var}=")
    }

    fn run_script(&self, f: &mut impl Write, path: &Path) -> std::fmt::Result {
        writeln!(f, "@CALL \"{}\"", path.to_string_lossy())
    }

    fn run_command<'a>(
        &self,
        f: &mut impl Write,
        command: impl IntoIterator<Item = &'a str> + 'a,
    ) -> std::fmt::Result {
        writeln!(f, "@{}", command.into_iter().join(" "))
    }

    fn extension(&self) -> &str {
        "bat"
    }

    fn executable(&self) -> &str {
        "cmd.exe"
    }

    fn create_run_script_command(&self, path: &Path) -> Command {
        let mut cmd = Command::new(self.executable());
        cmd.arg("/D").arg("/C").arg(path);
        cmd
    }

    fn format_env_var(&self, var_name: &str) -> String {
        format!("%{var_name}%")
    }

    fn echo(&self, f: &mut impl Write, text: &str) -> std::fmt::Result {
        write!(f, "@ECHO ",)?;

        // Escape special characters (see https://ss64.com/nt/syntax-esc.html)
        let mut text = text;
        while let Some(idx) = text.find(['^', '&', '|', '\\', '<', '>']) {
            write!(f, "{}^{}", &text[..idx], &text[idx..idx + 1])?;
            text = &text[idx + 1..];
        }
        writeln!(f, "{text}")
    }

    fn write_script(&self, f: &mut impl std::io::Write, script: &str) -> std::io::Result<()> {
        let script = script.replace('\n', "\r\n");
        f.write_all(script.as_bytes())
    }

    fn print_env(&self, f: &mut impl Write) -> std::fmt::Result {
        writeln!(f, "@SET")
    }

    fn line_ending(&self) -> &str {
        "\r\n"
    }
}

/// A [`Shell`] implementation for `PowerShell`.
#[derive(Debug, Clone)]
pub struct PowerShell {
    executable_path: String,
}

impl Default for PowerShell {
    fn default() -> Self {
        // Check if the modern "pwsh" PowerShell Core is available
        let test_powershell = Command::new("pwsh").arg("-v").output().is_ok();
        let exe = if test_powershell {
            "pwsh"
        } else {
            // Fall back to older "Windows PowerShell"
            "powershell"
        };

        PowerShell {
            executable_path: exe.to_string(),
        }
    }
}

impl Shell for PowerShell {
    fn force_utf8(&self, f: &mut impl Write) -> std::fmt::Result {
        // Taken from https://stackoverflow.com/a/49481797
        writeln!(f, "$OutputEncoding = [System.Console]::OutputEncoding = [System.Console]::InputEncoding = [System.Text.Encoding]::UTF8")
    }

    fn set_env_var(&self, f: &mut impl Write, env_var: &str, value: &str) -> std::fmt::Result {
        writeln!(f, "${{Env:{env_var}}} = \"{value}\"")
    }

    fn unset_env_var(&self, f: &mut impl Write, env_var: &str) -> std::fmt::Result {
        writeln!(f, "${{Env:{env_var}}}=\"\"")
    }

    fn run_script(&self, f: &mut impl Write, path: &Path) -> std::fmt::Result {
        writeln!(f, ". \"{}\"", path.to_string_lossy())
    }

    fn extension(&self) -> &str {
        "ps1"
    }

    fn executable(&self) -> &str {
        &self.executable_path
    }

    fn create_run_script_command(&self, path: &Path) -> Command {
        let mut cmd = Command::new(self.executable());
        cmd.arg(path);
        cmd
    }

    fn format_env_var(&self, var_name: &str) -> String {
        format!("$Env:{var_name}")
    }

    /// Emits writing all current environment variables to stdout.
    fn print_env(&self, f: &mut impl Write) -> std::fmt::Result {
        writeln!(f, r##"dir env: | %{{"{{0}}={{1}}" -f $_.Name,$_.Value}}"##)
    }
}

/// A [`Shell`] implementation for the Fish shell.
#[derive(Debug, Clone, Copy, Default)]
pub struct Fish;

impl Shell for Fish {
    fn set_env_var(&self, f: &mut impl Write, env_var: &str, value: &str) -> std::fmt::Result {
        writeln!(f, "set -gx {env_var} \"{value}\"")
    }

    fn format_env_var(&self, var_name: &str) -> String {
        // Fish doesnt want the extra brackets '{}'
        format!("${var_name}")
    }

    fn unset_env_var(&self, f: &mut impl Write, env_var: &str) -> std::fmt::Result {
        writeln!(f, "set -e {env_var}")
    }

    fn run_script(&self, f: &mut impl Write, path: &Path) -> std::fmt::Result {
        writeln!(f, "source \"{}\"", path.to_string_lossy())
    }

    fn extension(&self) -> &str {
        "fish"
    }

    fn executable(&self) -> &str {
        "fish"
    }

    fn create_run_script_command(&self, path: &Path) -> Command {
        let mut cmd = Command::new(self.executable());
        cmd.arg(path);
        cmd
    }
}

fn escape_backslashes(s: &str) -> String {
    s.replace('\\', "\\\\")
}
fn quote_if_required(s: &str) -> Cow<'_, str> {
    if s.contains(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-') {
        Cow::Owned(format!("\"{s}\""))
    } else {
        Cow::Borrowed(s)
    }
}

/// A [`Shell`] implementation for the Bash shell.
#[derive(Debug, Clone, Copy, Default)]
pub struct NuShell;

impl Shell for NuShell {
    fn set_env_var(&self, f: &mut impl Write, env_var: &str, value: &str) -> std::fmt::Result {
        // escape backslashes for Windows (make them double backslashes)
        writeln!(
            f,
            "$env.{} = \"{}\"",
            quote_if_required(env_var),
            escape_backslashes(value)
        )
    }

    fn unset_env_var(&self, f: &mut impl Write, env_var: &str) -> std::fmt::Result {
        writeln!(f, "hide-env {}", quote_if_required(env_var))
    }

    fn run_script(&self, f: &mut impl Write, path: &Path) -> std::fmt::Result {
        writeln!(f, "source-env \"{}\"", path.to_string_lossy())
    }

    fn set_path(
        &self,
        f: &mut impl Write,
        paths: &[PathBuf],
        modification_behavior: PathModificationBehavior,
        platform: &Platform,
    ) -> std::fmt::Result {
        let path = paths
            .iter()
            .map(|path| escape_backslashes(&format!("\"{}\"", path.to_string_lossy().into_owned())))
            .join(", ");

        // Replace, Append, or Prepend the path variable to the paths.
        let path_var = self.path_var(platform);
        match modification_behavior {
            PathModificationBehavior::Replace => {
                writeln!(f, "$env.{path_var} = [{path}]",)
            }
            PathModificationBehavior::Prepend => {
                writeln!(f, "$env.{path_var} = ($env.{path_var} | prepend [{path}])")
            }
            PathModificationBehavior::Append => {
                writeln!(f, "$env.{path_var} = ($env.{path_var} | append [{path}])")
            }
        }
    }

    fn extension(&self) -> &str {
        "nu"
    }

    fn executable(&self) -> &str {
        "nu"
    }

    fn create_run_script_command(&self, path: &Path) -> Command {
        let mut cmd = Command::new(self.executable());
        cmd.arg(path);
        cmd
    }
}

/// A generic [`Shell`] implementation for concrete shell types.
#[enum_dispatch]
#[allow(missing_docs)]
#[derive(Clone, Debug)]
pub enum ShellEnum {
    Bash,
    Zsh,
    Xonsh,
    CmdExe,
    PowerShell,
    Fish,
    NuShell,
}

// The default shell is determined by the current OS.
impl Default for ShellEnum {
    fn default() -> Self {
        if cfg!(windows) {
            CmdExe.into()
        } else {
            Bash.into()
        }
    }
}

impl ShellEnum {
    /// Parse a shell from a path to the executable for the shell.
    pub fn from_shell_path<P: AsRef<Path>>(path: P) -> Option<Self> {
        parse_shell_from_path(path.as_ref())
    }

    /// Determine the user's current shell from the environment
    ///
    /// This will read the SHELL environment variable and try to determine which
    /// shell is in use from that.
    ///
    /// If SHELL is set, but contains a value that doesn't correspond to one of
    /// the supported shell types, then return `None`.
    pub fn from_env() -> Option<Self> {
        if let Some(env_shell) = std::env::var_os("SHELL") {
            Self::from_shell_path(env_shell)
        } else if cfg!(windows) {
            Some(PowerShell::default().into())
        } else {
            None
        }
    }

    /// Guesses the current shell by checking the name of the parent process.
    #[cfg(feature = "sysinfo")]
    pub fn from_parent_process() -> Option<Self> {
        use sysinfo::get_current_pid;

        let mut system_info = sysinfo::System::new();

        // Get current process information
        let mut current_pid = get_current_pid().ok()?;
        system_info.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[current_pid]), true);

        while let Some(parent_process_id) = system_info
            .process(current_pid)
            .and_then(sysinfo::Process::parent)
        {
            // Get the name of the parent process
            system_info
                .refresh_processes(sysinfo::ProcessesToUpdate::Some(&[parent_process_id]), true);
            let parent_process = system_info.process(parent_process_id)?;
            let parent_process_name = parent_process
                .name()
                .to_string_lossy()
                .to_lowercase()
                .to_string();

            let shell: Option<ShellEnum> = if parent_process_name.contains("bash") {
                Some(Bash.into())
            } else if parent_process_name.contains("zsh") {
                Some(Zsh.into())
            } else if parent_process_name.contains("xonsh")
                // xonsh is a python shell, so we need to check if the parent process is python and if it
                // contains xonsh in the arguments.
                || (parent_process_name.contains("python")
                && parent_process
                .cmd().iter()
                .any(|arg| arg.to_string_lossy().contains("xonsh")))
            {
                Some(Xonsh.into())
            } else if parent_process_name.contains("fish") {
                Some(Fish.into())
            } else if parent_process_name.contains("nu") {
                Some(NuShell.into())
            } else if parent_process_name.contains("powershell")
                || parent_process_name.contains("pwsh")
            {
                Some(
                    PowerShell {
                        executable_path: parent_process_name.to_string(),
                    }
                    .into(),
                )
            } else if parent_process_name.contains("cmd.exe") {
                Some(CmdExe.into())
            } else {
                None
            };

            if let Some(shell) = shell {
                tracing::debug!(
                    "Guessing the current shell is {}. Parent process name: {} and args: {:?}",
                    &shell.executable(),
                    &parent_process_name,
                    &parent_process.cmd()
                );
                return Some(shell);
            }

            current_pid = parent_process_id;
        }

        None
    }
}

/// Parsing of a shell was not possible. The shell mostlikely is not supported.
#[derive(Debug, Error)]
#[error("{0}")]
pub struct ParseShellEnumError(String);

impl FromStr for ShellEnum {
    type Err = ParseShellEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bash" => Ok(Bash.into()),
            "zsh" => Ok(Zsh.into()),
            "xonsh" => Ok(Xonsh.into()),
            "fish" => Ok(Fish.into()),
            "cmd" => Ok(CmdExe.into()),
            "nu" | "nushell" => Ok(NuShell.into()),
            "powershell" | "powershell_ise" => Ok(PowerShell::default().into()),
            _ => Err(ParseShellEnumError(format!(
                "'{s}' is an unknown shell variant"
            ))),
        }
    }
}

/// Determine the shell from a path to a shell.
fn parse_shell_from_path(path: &Path) -> Option<ShellEnum> {
    let name = path.file_stem()?.to_str()?;
    ShellEnum::from_str(name).ok()
}

/// A helper struct for generating shell scripts.
pub struct ShellScript<T: Shell> {
    /// The shell class to generate the script for.
    shell: T,
    /// The contents of the script.
    contents: String,
    /// The platform for which the script will be generated
    platform: Platform,
}

impl<T: Shell + 'static> ShellScript<T> {
    /// Create a new [`ShellScript`] for the given shell.
    pub fn new(shell: T, platform: Platform) -> Self {
        Self {
            shell,
            contents: String::new(),
            platform,
        }
    }

    /// Export an environment variable.
    pub fn set_env_var(
        &mut self,
        env_var: &str,
        value: &str,
    ) -> Result<&mut Self, std::fmt::Error> {
        self.shell.set_env_var(&mut self.contents, env_var, value)?;
        Ok(self)
    }

    /// Unset an environment variable.
    pub fn unset_env_var(&mut self, env_var: &str) -> Result<&mut Self, std::fmt::Error> {
        self.shell.unset_env_var(&mut self.contents, env_var)?;
        Ok(self)
    }

    /// Set the PATH environment variable to the given paths.
    pub fn set_path(
        &mut self,
        paths: &[PathBuf],
        path_modification_behavior: PathModificationBehavior,
    ) -> Result<&mut Self, std::fmt::Error> {
        self.shell.set_path(
            &mut self.contents,
            paths,
            path_modification_behavior,
            &self.platform,
        )?;
        Ok(self)
    }

    /// Run a script in the generated shell script.
    pub fn run_script(&mut self, path: &Path) -> Result<&mut Self, std::fmt::Error> {
        self.shell.run_script(&mut self.contents, path)?;
        Ok(self)
    }

    /// Add contents to the script. The contents will be added as is, so make
    /// sure to format it correctly for the shell.
    pub fn append_script(&mut self, script: &Self) -> &mut Self {
        self.contents.push('\n');
        self.contents.push_str(&script.contents);
        self
    }

    /// Return the contents of the script.
    pub fn contents(&self) -> Result<String, std::fmt::Error> {
        let mut final_contents = String::new();
        self.shell.force_utf8(&mut final_contents)?;
        final_contents.push_str(&self.contents);

        if self.shell.line_ending() == "\n" {
            Ok(final_contents)
        } else {
            Ok(final_contents.replace('\n', self.shell.line_ending()))
        }
    }

    /// Print all environment variables to stdout during execution.
    pub fn print_env(&mut self) -> Result<&mut Self, std::fmt::Error> {
        self.shell.print_env(&mut self.contents)?;
        Ok(self)
    }

    /// Run `echo` in the shell script.
    pub fn echo(&mut self, text: &str) -> Result<&mut Self, std::fmt::Error> {
        self.shell.echo(&mut self.contents, text)?;
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_bash() {
        let mut script = ShellScript::new(Bash, Platform::Linux64);

        script
            .set_env_var("FOO", "bar")
            .unwrap()
            .unset_env_var("FOO")
            .unwrap()
            .run_script(&PathBuf::from_str("foo.sh").unwrap())
            .unwrap();

        insta::assert_snapshot!(script.contents);
    }

    #[test]
    fn test_fish() {
        let mut script = ShellScript::new(Fish, Platform::Linux64);

        script
            .set_env_var("FOO", "bar")
            .unwrap()
            .unset_env_var("FOO")
            .unwrap()
            .run_script(&PathBuf::from_str("foo.sh").expect("blah"))
            .unwrap();

        insta::assert_snapshot!(script.contents);
    }

    #[test]
    fn test_xonsh_bash() {
        let mut script = ShellScript::new(Xonsh, Platform::Linux64);

        script
            .run_script(&PathBuf::from_str("foo.sh").unwrap())
            .unwrap();

        insta::assert_snapshot!(script.contents);
    }

    #[test]
    fn test_xonsh_xsh() {
        let mut script = ShellScript::new(Xonsh, Platform::Linux64);
        script
            .set_env_var("FOO", "bar")
            .unwrap()
            .unset_env_var("FOO")
            .unwrap()
            .run_script(&PathBuf::from_str("foo.xsh").unwrap())
            .unwrap();

        insta::assert_snapshot!(script.contents);
    }

    #[cfg(feature = "sysinfo")]
    #[test]
    fn test_from_parent_process_doenst_crash() {
        let shell = ShellEnum::from_parent_process();
        println!("Detected shell: {shell:?}");
    }

    #[test]
    fn test_from_env() {
        let shell = ShellEnum::from_env();
        println!("Detected shell: {shell:?}");
    }

    #[test]
    fn test_path_separator() {
        let mut script = ShellScript::new(Bash, Platform::Linux64);
        script
            .set_path(
                &[PathBuf::from("/foo"), PathBuf::from("/bar")],
                PathModificationBehavior::Prepend,
            )
            .unwrap();
        assert!(script.contents.contains("/foo:/bar"));

        let mut script = ShellScript::new(Bash, Platform::Win64);
        script
            .set_path(
                &[PathBuf::from("/foo"), PathBuf::from("/bar")],
                PathModificationBehavior::Prepend,
            )
            .unwrap();
        assert!(script.contents.contains("/foo;/bar"));
    }

    #[test]
    fn test_parse_env() {
        let script = ShellScript::new(CmdExe, Platform::Win64);
        let input = "VAR1=\"value1\"\nNUM=1\nNUM2=\"2\"";
        let parsed_env = script.shell.parse_env(input);

        let expected_env: HashMap<&str, &str> =
            vec![("VAR1", "value1"), ("NUM", "1"), ("NUM2", "2")]
                .into_iter()
                .collect();

        assert_eq!(parsed_env, expected_env);
    }
}
