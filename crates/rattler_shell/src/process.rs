//! Running a child process within a time and an output bound.
//!
//! Both the detector executable and the shell that activates its environment
//! run through [`run_bounded`]: the clock starts when the process was spawned,
//! standard output and standard error are read concurrently against one
//! combined byte budget, and when either bound is reached the process and its
//! descendants are terminated. Standard error captured until then is kept for
//! diagnostics.

use std::{
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};

use thiserror::Error;
use tokio::io::AsyncReadExt;

/// The bounds of one process run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessLimits {
    /// How long the process may run after it was spawned.
    pub timeout: Duration,
    /// How many bytes of standard output and standard error, combined, the
    /// process may write.
    pub output_limit: usize,
}

/// A process that exited within the limits, with whatever status.
#[derive(Clone, Debug)]
pub struct ProcessOutput {
    /// The exit status.
    pub status: ExitStatus,
    /// Everything the process wrote to standard output.
    pub stdout: Vec<u8>,
    /// Everything the process wrote to standard error.
    pub stderr: Vec<u8>,
    /// How long the process ran.
    pub duration: Duration,
}

/// Why a process could not be run to completion within the limits.
#[derive(Debug, Error)]
pub enum ProcessError {
    /// The process could not be started.
    #[error("failed to start the process")]
    Spawn(#[source] std::io::Error),

    /// The process's output could not be read or its exit could not be
    /// awaited.
    #[error("failed to read the process output")]
    Io {
        /// The underlying error.
        #[source]
        source: std::io::Error,
        /// Standard error captured before the failure.
        stderr: Vec<u8>,
    },

    /// The process did not exit within the timeout and was terminated.
    #[error("the process did not finish within {timeout:?} and was terminated")]
    TimedOut {
        /// The timeout that was reached.
        timeout: Duration,
        /// Standard error captured before termination.
        stderr: Vec<u8>,
    },

    /// The process reached the combined output limit and was terminated.
    #[error("the process reached the {limit}-byte output limit and was terminated")]
    OutputLimitExceeded {
        /// The limit that was exceeded.
        limit: usize,
        /// Standard error captured before termination.
        stderr: Vec<u8>,
    },
}

impl ProcessError {
    /// Standard error captured before the run failed, where a process ran at
    /// all.
    pub fn stderr(&self) -> Option<&[u8]> {
        match self {
            Self::Spawn(_) => None,
            Self::Io { stderr, .. }
            | Self::TimedOut { stderr, .. }
            | Self::OutputLimitExceeded { stderr, .. } => Some(stderr),
        }
    }
}

/// Runs `command` with no standard input and both output streams captured,
/// within `limits`.
pub async fn run_bounded(
    command: &mut tokio::process::Command,
    limits: ProcessLimits,
) -> Result<ProcessOutput, ProcessError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn().map_err(ProcessError::Spawn)?;
    let started = Instant::now();
    let mut tree = ProcessTree::new(&child).map_err(ProcessError::Spawn)?;
    let mut stdout_pipe = child.stdout.take().expect("stdout is piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr is piped");

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let read_all = async {
        let mut stdout_buf = [0u8; 8192];
        let mut stderr_buf = [0u8; 8192];
        let mut stdout_open = true;
        let mut stderr_open = true;
        while stdout_open || stderr_open {
            let remaining = limits.output_limit - stdout.len() - stderr.len();
            if remaining == 0 {
                return Ok::<bool, std::io::Error>(false);
            }
            let capacity = remaining.min(stdout_buf.len());
            let read = tokio::select! {
                result = stdout_pipe.read(&mut stdout_buf[..capacity]), if stdout_open => match result? {
                    0 => { stdout_open = false; 0 }
                    n => { stdout.extend_from_slice(&stdout_buf[..n]); n }
                },
                result = stderr_pipe.read(&mut stderr_buf[..capacity]), if stderr_open => match result? {
                    0 => { stderr_open = false; 0 }
                    n => { stderr.extend_from_slice(&stderr_buf[..n]); n }
                },
            };
            if read > 0 && stdout.len() + stderr.len() >= limits.output_limit {
                return Ok::<bool, std::io::Error>(false);
            }
        }
        Ok(true)
    };

    let within_limits = match tokio::time::timeout(limits.timeout, read_all).await {
        Ok(Ok(within_limits)) => within_limits,
        Ok(Err(source)) => {
            tree.kill(&mut child).await;
            return Err(ProcessError::Io { source, stderr });
        }
        Err(_) => {
            tree.kill(&mut child).await;
            return Err(ProcessError::TimedOut {
                timeout: limits.timeout,
                stderr,
            });
        }
    };
    if !within_limits {
        tree.kill(&mut child).await;
        return Err(ProcessError::OutputLimitExceeded {
            limit: limits.output_limit,
            stderr,
        });
    }

    // The pipes are closed, so the process is exiting or has exited; the wait
    // still counts against the timeout.
    let remaining = limits.timeout.saturating_sub(started.elapsed());
    let Ok(status) = tokio::time::timeout(remaining, child.wait()).await else {
        tree.kill(&mut child).await;
        return Err(ProcessError::TimedOut {
            timeout: limits.timeout,
            stderr,
        });
    };
    let status = status.map_err(|source| ProcessError::Io {
        source,
        stderr: stderr.clone(),
    })?;
    #[cfg(unix)]
    {
        tree.pgid = None;
    }
    Ok(ProcessOutput {
        status,
        stdout,
        stderr,
        duration: started.elapsed(),
    })
}

/// A handle on a process and its descendants, so that all of them can be
/// terminated together.
struct ProcessTree {
    #[cfg(unix)]
    pgid: Option<libc::pid_t>,
    #[cfg(windows)]
    job: windows::JobObject,
}

impl ProcessTree {
    fn new(child: &tokio::process::Child) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                pgid: child.id().and_then(|id| libc::pid_t::try_from(id).ok()),
            })
        }
        #[cfg(windows)]
        {
            Ok(Self {
                job: windows::JobObject::assign(child)?,
            })
        }
    }

    fn terminate(&mut self) {
        #[cfg(unix)]
        if let Some(pgid) = self.pgid.take() {
            // SAFETY: `killpg` has no memory-safety preconditions; the group id
            // is the one the child was spawned in.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        self.job.terminate();
    }

    /// Terminates the process group or job, then reaps the child.
    async fn kill(&mut self, child: &mut tokio::process::Child) {
        self.terminate();
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}

#[cfg(unix)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject,
        },
    };

    /// A job object that kills every process in it when it is closed.
    ///
    /// The child is assigned right after it was spawned. A child that starts
    /// its own children before that moment, which a batch file run through
    /// `cmd.exe` can in principle do, is not covered by the job; that is why
    /// terminating descendants is only a SHOULD.
    pub(super) struct JobObject(HANDLE);

    // SAFETY: a job object handle can be used from any thread.
    unsafe impl Send for JobObject {}
    unsafe impl Sync for JobObject {}

    impl JobObject {
        pub(super) fn assign(child: &tokio::process::Child) -> std::io::Result<Self> {
            // SAFETY: plain Win32 calls with valid arguments; the handle is
            // closed in `Drop`.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return Err(std::io::Error::last_os_error());
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    std::ptr::addr_of!(info).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) == 0
                {
                    let error = std::io::Error::last_os_error();
                    CloseHandle(job);
                    return Err(error);
                }
                if let Some(process) = child.raw_handle()
                    && AssignProcessToJobObject(job, process as HANDLE) == 0
                {
                    let error = std::io::Error::last_os_error();
                    CloseHandle(job);
                    return Err(error);
                }
                Ok(Self(job))
            }
        }

        pub(super) fn terminate(&self) {
            // SAFETY: the handle is a valid job object owned by `self`.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for JobObject {
        fn drop(&mut self) {
            // SAFETY: the handle is a valid job object owned by `self`.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        os::{fd::AsRawFd, unix::net::UnixStream},
    };

    use super::*;

    fn sh(script: &str) -> tokio::process::Command {
        let mut command = tokio::process::Command::new("sh");
        command.arg("-c").arg(script);
        command
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_a_run_terminates_its_descendants() {
        struct Cleanup {
            task: tokio::task::JoinHandle<Result<ProcessOutput, ProcessError>>,
            pgid: Option<libc::pid_t>,
        }

        impl Drop for Cleanup {
            fn drop(&mut self) {
                self.task.abort();
                if let Some(pgid) = self.pgid {
                    // SAFETY: this is the dedicated group created for the
                    // fixture, kept until all assertions and cleanup finish.
                    unsafe {
                        libc::killpg(pgid, libc::SIGKILL);
                    }
                }
            }
        }

        let (parent_socket, child_socket) = UnixStream::pair().unwrap();
        parent_socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let descriptor = child_socket.as_raw_fd();
        let mut command = sh("printf '%s\\n' \"$$\" >&3\n\
             read gate <&3\n\
             sleep 300 &\n\
             printf 'ready\\n' >&3\n\
             wait");
        // SAFETY: only async-signal-safe descriptor operations run after
        // fork. The owning socket stays alive until the command is spawned.
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(descriptor, 3) == -1 || libc::fcntl(3, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut cleanup = Cleanup {
            task: tokio::spawn(async move {
                let _socket = child_socket;
                run_bounded(
                    &mut command,
                    ProcessLimits {
                        timeout: Duration::from_secs(60),
                        output_limit: 1024,
                    },
                )
                .await
            }),
            pgid: None,
        };
        let mut reader = BufReader::new(parent_socket);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        cleanup.pgid = Some(line.trim().parse().unwrap());
        // No descendants are started until the fallback cleanup owns the
        // process group, including when the expected regression fails.
        reader.get_mut().write_all(b"go\n").unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "ready\n");

        cleanup.task.abort();
        assert!((&mut cleanup.task).await.unwrap_err().is_cancelled());
        let eof = reader.read_to_end(&mut Vec::new());
        drop(cleanup);
        assert_eq!(
            eof.expect("a descendant kept its inherited socket open after cancellation"),
            0
        );
    }

    #[tokio::test]
    async fn captures_both_streams_and_the_status() {
        let output = run_bounded(
            &mut sh("echo out; echo err >&2; exit 3"),
            ProcessLimits {
                timeout: Duration::from_secs(10),
                output_limit: 1024,
            },
        )
        .await
        .unwrap();
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(output.stdout, b"out\n");
        assert_eq!(output.stderr, b"err\n");
    }

    #[tokio::test]
    async fn timeout_terminates_the_process_group() {
        // The background child keeps the pipes open; it must be killed too.
        let started = Instant::now();
        let err = run_bounded(
            &mut sh("echo starting >&2; sleep 30 & sleep 30"),
            ProcessLimits {
                timeout: Duration::from_millis(300),
                output_limit: 1024,
            },
        )
        .await
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5));
        match err {
            ProcessError::TimedOut { stderr, .. } => assert_eq!(stderr, b"starting\n"),
            other => panic!("unexpected error {other}"),
        }
    }

    #[tokio::test]
    async fn output_budget_stops_at_the_combined_limit() {
        let limits = ProcessLimits {
            timeout: Duration::from_secs(10),
            output_limit: 4,
        };
        let below = run_bounded(&mut sh("printf 123 >&2"), limits)
            .await
            .unwrap();
        assert_eq!(below.stderr, b"123");
        for (script, expected_stderr) in [
            ("printf 1234 >&2", b"1234".as_slice()),
            ("printf 12345 >&2", b"1234".as_slice()),
            ("printf 12; printf 34 >&2", b"34".as_slice()),
        ] {
            match run_bounded(&mut sh(script), limits).await.unwrap_err() {
                ProcessError::OutputLimitExceeded { limit, stderr } => {
                    assert_eq!(limit, 4);
                    assert_eq!(stderr, expected_stderr);
                }
                other => panic!("unexpected error {other}"),
            }
        }
    }

    #[tokio::test]
    async fn output_limit_terminates_the_process() {
        let err = run_bounded(
            &mut sh("echo noisy >&2; while true; do echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa; done"),
            ProcessLimits {
                timeout: Duration::from_secs(20),
                output_limit: 4096,
            },
        )
        .await
        .unwrap_err();
        match err {
            ProcessError::OutputLimitExceeded { limit, stderr } => {
                assert_eq!(limit, 4096);
                assert!(stderr.len() <= limit);
            }
            other => panic!("unexpected error {other}"),
        }
    }
}
