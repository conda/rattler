//! Running a plugin's entry point out of the environment it was installed in.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use rattler_conda_types::Platform;
use rattler_shell::activation::prefix_path_entries;
use tokio::io::AsyncReadExt;

/// How long a plugin may run before it is killed.
///
/// Detection happens on the way into a solve, and a plugin that hangs would
/// hang the solve with it. How long is long enough is not something this crate
/// can know, though: a plugin reading a version file is done in microseconds,
/// while one connecting to a GPU has been measured at over a second on Windows.
/// So the bound is the caller's to set, and only the caller's -- a plugin cannot
/// ask for more time.
///
/// There is a ceiling the caller cannot pass either. A value only ever reaches
/// this type through [`RunTimeout::new`], which clamps, so no timeout anywhere
/// can exceed [`RunTimeout::MAX`] -- not by configuration, not by a caller's
/// arithmetic, not by a channel talking someone into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunTimeout(Duration);

impl RunTimeout {
    /// What a caller that has no opinion gets.
    ///
    /// Five seconds. One second is provably too short: `__cuda` on Windows has
    /// been measured at a second and a half, because it has to connect to the
    /// GPU. Five leaves that room on cold hardware while still being short
    /// enough that a hung plugin is noticed rather than waited out.
    pub const DEFAULT: Duration = Duration::from_secs(5);

    /// The longest any plugin may be given, whatever a caller asks for.
    ///
    /// A minute is far past anything detection should need, so a plugin that
    /// hits this is hung rather than slow. The ceiling exists because the cost
    /// of getting the timeout wrong is asymmetric: too short only skips a
    /// plugin, while unbounded stalls every solve on the machine.
    pub const MAX: Duration = Duration::from_secs(60);

    /// A bound of `timeout`, clamped to [`RunTimeout::MAX`].
    ///
    /// Clamping rather than refusing: a caller asking for ten minutes has
    /// misjudged how long detection takes, and running with a minute is a
    /// better answer than an error about a number.
    pub fn new(timeout: Duration) -> Self {
        if timeout > Self::MAX {
            tracing::debug!(
                "a plugin timeout of {timeout:?} was asked for; using the maximum of {:?}",
                Self::MAX
            );
        }
        Self(timeout.min(Self::MAX))
    }

    /// How long a plugin may run. Never more than [`RunTimeout::MAX`].
    pub fn get(self) -> Duration {
        self.0
    }
}

impl Default for RunTimeout {
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}

/// The most a well-behaved plugin can need to write about one virtual package.
///
/// A verdict cannot get long: a conda package's name, version and build string
/// together fit in an archive file name, which caps the three at under 250
/// bytes before the JSON around them. What can get long is a watched filesystem
/// path, at most `PATH_MAX` (4096 bytes on Linux); this fits one maximal path
/// even with every byte JSON-escaped.
pub const MAX_BYTES_PER_VIRTUAL_PACKAGE: usize = 8 * 1024;

/// The most output a plugin registered for `declared_count` virtual packages
/// may produce, counted across stdout and stderr together.
///
/// One verdict's worth per registered virtual package, one for the cache policy,
/// and one of slack. Legitimate output is nowhere near this; without a bound, a
/// misbehaving plugin gets handed the client's memory.
pub fn output_budget(declared_count: usize) -> usize {
    MAX_BYTES_PER_VIRTUAL_PACKAGE.saturating_mul(declared_count.saturating_add(2))
}

/// What a plugin run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRun {
    /// Everything the plugin wrote to stdout, to be handed to
    /// [`parse_report`](crate::parse_report).
    pub stdout: String,

    /// Everything the plugin wrote to stderr. Diagnostics, for logging.
    pub stderr: String,

    /// The process exit code, or `None` if a signal killed it.
    ///
    /// Anything but `Some(0)` means the run failed and every virtual package the
    /// plugin was registered for has to be treated as absent.
    pub exit_code: Option<i32>,
}

impl PluginRun {
    /// Whether the plugin ran to completion, making its output authoritative.
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// A plugin could not be run at all, as distinct from one that ran and failed.
#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    /// The plugin's environment could not be activated.
    #[error(transparent)]
    Activation(#[from] crate::activation::ActivationError),

    /// No executable named after the plugin package exists in the environment.
    #[error("'{entry_point}' is not in the plugin environment at '{}'", prefix.display())]
    EntryPointMissing {
        /// The executable that was looked for.
        entry_point: String,
        /// The prefix that was searched.
        prefix: PathBuf,
    },

    /// The executable exists but could not be started.
    #[error("failed to run '{}'", executable.display())]
    Spawn {
        /// The executable that could not be started.
        executable: PathBuf,
        /// Why it could not be started.
        #[source]
        source: std::io::Error,
    },

    /// The plugin was still running when its [`RunTimeout`] elapsed.
    #[error("'{}' was still running after {timeout:?} and was killed", executable.display())]
    TimedOut {
        /// The executable that was killed.
        executable: PathBuf,
        /// How long it was given.
        timeout: Duration,
        /// What it had written to stderr by then.
        stderr: String,
    },

    /// The plugin wrote more than [`output_budget`] allows for its
    /// registration.
    #[error("'{}' produced more than {budget} bytes of output and was killed", executable.display())]
    TooMuchOutput {
        /// The executable that was killed.
        executable: PathBuf,
        /// The budget it exceeded, in bytes.
        budget: usize,
        /// What it had written to stderr by then.
        stderr: String,
    },

    /// The plugin's output could not be read.
    #[error("failed to read the output of '{}'", executable.display())]
    Read {
        /// The executable whose output was being read.
        executable: PathBuf,
        /// The underlying I/O failure.
        #[source]
        source: std::io::Error,
        /// What the plugin had written to stderr before the read failed.
        stderr: String,
    },
}

impl RunnerError {
    /// What the plugin wrote to stderr before it was killed, where there was a
    /// plugin writing anything.
    ///
    /// A plugin's own diagnostics are usually the only explanation of why it
    /// hung or flooded its budget, so they have to survive the failure rather
    /// than be dropped with the output that was being collected.
    pub fn plugin_stderr(&self) -> Option<&str> {
        match self {
            Self::Activation(_) | Self::EntryPointMissing { .. } | Self::Spawn { .. } => None,
            Self::TimedOut { stderr, .. }
            | Self::TooMuchOutput { stderr, .. }
            | Self::Read { stderr, .. } => Some(stderr),
        }
    }
}

/// Everything needed to run one plugin.
pub struct RunOptions<'a> {
    /// The prefix the plugin was installed into.
    pub prefix: &'a Path,

    /// The executable to run, which is named after the plugin package.
    pub entry_point: &'a str,

    /// The platform whose binary directory layout the prefix follows.
    pub platform: Platform,

    /// How many virtual packages the channel registered the plugin for. The
    /// output budget is derived from it.
    pub declared_count: usize,

    /// How long the plugin may run.
    pub timeout: RunTimeout,
}

/// Runs a plugin's entry point out of its prefix and collects what it said.
///
/// The prefix is activated first, and the plugin is then invoked **directly**
/// with the environment activation produced -- not as a command inside an
/// activated shell. It gets what any other program in a conda environment gets,
/// while anything the activation scripts print stays on the activating shell's
/// stdout, where it cannot be mistaken for part of the report.
///
/// The whole thing is bounded by one [`RunTimeout`], activation included: a
/// caller allowing five seconds is allowing five seconds to get an answer, not
/// five for each half. A plugin still running when that elapses, or one
/// producing more than [`output_budget`]`(declared_count)` bytes of output, is
/// killed and reported as an error of its own -- carrying whatever it wrote to
/// stderr first, since that is usually the only account of why. A non-zero exit
/// is reported in [`PluginRun::exit_code`] rather than as an error: the plugin
/// ran, it just failed, and the caller decides what that means.
pub async fn run_plugin(options: RunOptions<'_>) -> Result<PluginRun, RunnerError> {
    let RunOptions {
        prefix,
        entry_point,
        platform,
        declared_count,
        timeout,
    } = options;
    let deadline = tokio::time::Instant::now() + timeout.get();

    // Looked up before activating: a registration naming a package that ships no
    // executable is worth saying at once rather than after a shell has run.
    let executable = find_entry_point(prefix, entry_point, platform).ok_or_else(|| {
        RunnerError::EntryPointMissing {
            entry_point: entry_point.to_string(),
            prefix: prefix.to_path_buf(),
        }
    })?;

    let activated =
        crate::activation::activated_environment(prefix, platform, deadline, timeout.get()).await?;

    let mut child = tokio::process::Command::new(&executable)
        // Activation sets both of these. They are set first anyway, so a prefix
        // whose activation changes neither still runs the plugin in its own
        // environment rather than in the caller's.
        .env("PATH", prefixed_path(prefix, platform))
        .env("CONDA_PREFIX", prefix)
        .envs(activated)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|source| RunnerError::Spawn {
            executable: executable.clone(),
            source,
        })?;

    // The buffers outlive the collecting, so a run that is killed part way still
    // has what the plugin said about itself.
    let budget = output_budget(declared_count);
    let mut streams = Streams::default();
    let collected =
        tokio::time::timeout_at(deadline, collect_output(&mut child, &mut streams, budget)).await;

    match collected {
        Ok(Ok(Collected::Complete { exit_code })) => Ok(PluginRun {
            stdout: text(&streams.stdout),
            stderr: text(&streams.stderr),
            exit_code,
        }),
        Ok(Ok(Collected::OverBudget)) => {
            kill(&mut child).await;
            Err(RunnerError::TooMuchOutput {
                executable,
                budget,
                stderr: text(&streams.stderr),
            })
        }
        Ok(Err(source)) => {
            kill(&mut child).await;
            Err(RunnerError::Read {
                executable,
                source,
                stderr: text(&streams.stderr),
            })
        }
        Err(_elapsed) => {
            kill(&mut child).await;
            Err(RunnerError::TimedOut {
                executable,
                timeout: timeout.get(),
                stderr: text(&streams.stderr),
            })
        }
    }
}

/// What a plugin has written so far.
///
/// Owned by the caller of [`collect_output`] rather than by it, so that a run
/// cut short by the timeout still leaves the bytes behind.
#[derive(Debug, Default)]
struct Streams {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// What a plugin wrote, as text. A plugin that writes something other than UTF-8
/// has a bug the protocol will report; mangling it beats refusing to say so.
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// What reading a plugin's output until EOF ended in.
enum Collected {
    /// The plugin finished within its budget.
    Complete {
        /// Its exit code, or `None` if a signal killed it.
        exit_code: Option<i32>,
    },
    /// The plugin exceeded its budget and must be killed.
    OverBudget,
}

/// Reads stdout and stderr into `streams`, then waits for the exit status.
///
/// The two streams are drained together: reading one to its end first would
/// deadlock against a plugin blocked on writing the other. Every byte counts
/// against `budget`, and exceeding it stops the reading immediately -- the
/// point of the budget is not to buffer what a misbehaving plugin writes.
async fn collect_output(
    child: &mut tokio::process::Child,
    streams: &mut Streams,
    budget: usize,
) -> std::io::Result<Collected> {
    let mut stdout = child.stdout.take().expect("stdout was configured as piped");
    let mut stderr = child.stderr.take().expect("stderr was configured as piped");
    let mut stdout_chunk = [0u8; 4096];
    let mut stderr_chunk = [0u8; 4096];
    let mut stdout_open = true;
    let mut stderr_open = true;

    while stdout_open || stderr_open {
        tokio::select! {
            read = stdout.read(&mut stdout_chunk), if stdout_open => match read? {
                0 => stdout_open = false,
                n => streams.stdout.extend_from_slice(&stdout_chunk[..n]),
            },
            read = stderr.read(&mut stderr_chunk), if stderr_open => match read? {
                0 => stderr_open = false,
                n => streams.stderr.extend_from_slice(&stderr_chunk[..n]),
            },
        }
        if streams.stdout.len() + streams.stderr.len() > budget {
            return Ok(Collected::OverBudget);
        }
    }

    let status = child.wait().await?;
    Ok(Collected::Complete {
        exit_code: status.code(),
    })
}

/// Kills the plugin process on the way to reporting an error.
///
/// A kill can only fail when the process is already gone, so the error being
/// reported alongside stays the interesting one.
async fn kill(child: &mut tokio::process::Child) {
    if let Err(error) = child.kill().await {
        tracing::debug!("failed to kill the plugin process: {error}");
    }
}

/// Locates the executable named after the plugin package, trying the
/// extensions Windows needs to consider one runnable.
fn find_entry_point(prefix: &Path, entry_point: &str, platform: Platform) -> Option<PathBuf> {
    let extensions: &[&str] = if platform.is_windows() {
        &["", ".exe", ".bat", ".cmd"]
    } else {
        &[""]
    };

    prefix_path_entries(prefix, &platform)
        .into_iter()
        .flat_map(|dir| {
            extensions
                .iter()
                .map(move |extension| dir.join(format!("{entry_point}{extension}")))
        })
        .find(|candidate| candidate.is_file())
}

/// The environment's binary directories ahead of the inherited `PATH`, so a
/// plugin finds its own helpers before anything on the host.
fn prefixed_path(prefix: &Path, platform: Platform) -> OsString {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let entries = prefix_path_entries(prefix, &platform)
        .into_iter()
        .chain(std::env::split_paths(&inherited));
    std::env::join_paths(entries).unwrap_or(inherited)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes an executable named `entry_point` into `prefix` running the
    /// platform's variant of the given script body.
    fn write_plugin_script(prefix: &Path, entry_point: &str, unix_body: &str, windows_body: &str) {
        let platform = Platform::current();
        let bin_dir = prefix_path_entries(prefix, &platform)
            .into_iter()
            .next()
            .expect("a platform always has at least one binary directory");
        std::fs::create_dir_all(&bin_dir).unwrap();

        if platform.is_windows() {
            std::fs::write(
                bin_dir.join(format!("{entry_point}.bat")),
                format!("@echo off\r\n{windows_body}"),
            )
            .unwrap();
        } else {
            let path = bin_dir.join(entry_point);
            std::fs::write(&path, format!("#!/bin/sh\n{unix_body}")).unwrap();

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
    }

    /// Writes an executable named `entry_point` into `prefix` that prints
    /// `stdout_lines` and exits with `exit_code`.
    fn write_fake_plugin(prefix: &Path, entry_point: &str, stdout_lines: &[&str], exit_code: i32) {
        let mut unix = String::new();
        let mut windows = String::new();
        for line in stdout_lines {
            unix.push_str(&format!("printf '%s\\n' '{line}'\n"));
            windows.push_str(&format!("echo {line}\r\n"));
        }
        unix.push_str(&format!("exit {exit_code}\n"));
        windows.push_str(&format!("exit /b {exit_code}\r\n"));
        write_plugin_script(prefix, entry_point, &unix, &windows);
    }

    /// Options for running `entry_point` out of `prefix` with the defaults.
    fn options<'a>(
        prefix: &'a Path,
        entry_point: &'a str,
        declared_count: usize,
    ) -> RunOptions<'a> {
        RunOptions {
            prefix,
            entry_point,
            platform: Platform::current(),
            declared_count,
            timeout: RunTimeout::default(),
        }
    }

    #[tokio::test]
    async fn captures_stdout_of_a_successful_run() {
        let prefix = tempfile::tempdir().unwrap();
        write_fake_plugin(
            prefix.path(),
            "cuda-detect",
            &[
                r#"{"version": 1, "virtual_packages": {"__cuda": {"version": "12.4"}, "__cuda_arch": null}}"#,
            ],
            0,
        );

        let run = run_plugin(options(prefix.path(), "cuda-detect", 2))
            .await
            .unwrap();

        assert!(run.succeeded());
        // Round-trips through the protocol, which is the point of capturing it.
        let report = crate::parse_report(&run.stdout).unwrap();
        assert_eq!(report.virtual_packages.len(), 2);
    }

    #[tokio::test]
    async fn a_failing_plugin_is_reported_not_raised() {
        let prefix = tempfile::tempdir().unwrap();
        write_fake_plugin(prefix.path(), "broken-detect", &["not json"], 3);

        let run = run_plugin(options(prefix.path(), "broken-detect", 1))
            .await
            .unwrap();

        assert!(!run.succeeded());
        assert_eq!(run.exit_code, Some(3));
        assert!(run.stdout.contains("not json"));
    }

    #[tokio::test]
    async fn a_missing_entry_point_is_an_error() {
        let prefix = tempfile::tempdir().unwrap();
        write_fake_plugin(prefix.path(), "cuda-detect", &[], 0);

        let err = run_plugin(options(prefix.path(), "rocm-detect", 1))
            .await
            .unwrap_err();

        assert!(
            matches!(err, RunnerError::EntryPointMissing { .. }),
            "{err}"
        );
        assert_eq!(
            err.plugin_stderr(),
            None,
            "nothing ran, so there is nothing it said"
        );
    }

    #[tokio::test]
    async fn a_hanging_plugin_is_killed_and_keeps_what_it_said() {
        let prefix = tempfile::tempdir().unwrap();
        write_plugin_script(
            prefix.path(),
            "hang-detect",
            "echo 'still waiting for the driver' >&2\nsleep 10\n",
            "echo still waiting for the driver 1>&2\r\nping -n 11 127.0.0.1 >nul\r\n",
        );

        let mut options = options(prefix.path(), "hang-detect", 1);
        options.timeout = RunTimeout::new(Duration::from_secs(1));
        let err = run_plugin(options).await.unwrap_err();

        assert!(matches!(err, RunnerError::TimedOut { .. }), "{err}");
        assert!(
            err.plugin_stderr()
                .is_some_and(|stderr| stderr.contains("still waiting for the driver")),
            "the plugin's own diagnostics were dropped: {:?}",
            err.plugin_stderr()
        );
    }

    #[tokio::test]
    async fn a_slow_plugin_finishes_when_the_caller_allows_for_it() {
        let prefix = tempfile::tempdir().unwrap();
        write_plugin_script(
            prefix.path(),
            "slow-detect",
            "sleep 0.5\nprintf '%s\\n' '{}'\n",
            "ping -n 2 127.0.0.1 >nul\r\necho {}\r\n",
        );

        let mut too_short = options(prefix.path(), "slow-detect", 0);
        too_short.timeout = RunTimeout::new(Duration::from_millis(100));
        assert!(
            matches!(
                run_plugin(too_short).await,
                Err(RunnerError::TimedOut { .. })
            ),
            "the default-length bound has to be the one being relaxed"
        );

        let mut long_enough = options(prefix.path(), "slow-detect", 0);
        long_enough.timeout = RunTimeout::new(Duration::from_secs(10));
        let run = run_plugin(long_enough).await.unwrap();
        assert!(run.succeeded(), "stderr: {}", run.stderr);
    }

    #[tokio::test]
    async fn a_plugin_exceeding_its_output_budget_is_killed() {
        let prefix = tempfile::tempdir().unwrap();
        let line = "x".repeat(1024);
        // Three times the budget for a plugin registered for nothing.
        let lines = vec![line.as_str(); 3 * output_budget(0) / 1024];
        write_fake_plugin(prefix.path(), "spew-detect", &lines, 0);

        let err = run_plugin(options(prefix.path(), "spew-detect", 0))
            .await
            .unwrap_err();

        assert!(matches!(err, RunnerError::TooMuchOutput { .. }), "{err}");
    }

    #[test]
    fn no_timeout_can_exceed_the_maximum() {
        assert_eq!(
            RunTimeout::new(Duration::from_secs(600)).get(),
            RunTimeout::MAX
        );
        assert_eq!(RunTimeout::new(Duration::MAX).get(), RunTimeout::MAX);
        assert_eq!(
            RunTimeout::new(RunTimeout::MAX).get(),
            RunTimeout::MAX,
            "asking for exactly the maximum is not over it"
        );
    }

    #[test]
    fn a_timeout_under_the_maximum_is_kept() {
        let asked = Duration::from_millis(1500);
        assert_eq!(RunTimeout::new(asked).get(), asked);
        assert_eq!(RunTimeout::default().get(), RunTimeout::DEFAULT);
        assert!(
            RunTimeout::DEFAULT < RunTimeout::MAX,
            "the default has to leave room to be raised"
        );
    }

    #[test]
    fn the_default_allows_for_a_gpu_query() {
        assert!(
            RunTimeout::DEFAULT > Duration::from_millis(1500),
            "the default must not cut off the case it was raised for"
        );
    }

    #[test]
    fn the_budget_scales_with_the_registration() {
        assert_eq!(output_budget(0), 2 * MAX_BYTES_PER_VIRTUAL_PACKAGE);
        assert_eq!(output_budget(3), 5 * MAX_BYTES_PER_VIRTUAL_PACKAGE);
    }

    #[test]
    fn the_environment_comes_first_on_path() {
        let prefix = Path::new("/tmp/does-not-need-to-exist");
        let path = prefixed_path(prefix, Platform::current());
        let first = std::env::split_paths(&path).next().unwrap();
        assert_eq!(
            first,
            prefix_path_entries(prefix, &Platform::current())[0],
            "the environment must precede the inherited PATH"
        );
    }
}
