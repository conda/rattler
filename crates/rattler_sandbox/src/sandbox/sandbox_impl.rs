//! We expose some sandbox items that then call itself through the trampoline
use std::process::Command;

/// Add exceptions to the sandbox
pub enum Exception {
    ExecuteAndRead(String),
    Read(String),
    ReadAndWrite(String),
    Networking,
}

/// Create a `Command` that will run the current executable with the given exceptions
pub fn sandboxed_command(exe: &str, exceptions: &[Exception]) -> Command {
    let self_exe = std::env::current_exe().unwrap();
    let mut cmd = Command::new(self_exe);
    cmd.arg("__sandbox_trampoline__");

    for exception in exceptions {
        match exception {
            Exception::ExecuteAndRead(path) => {
                cmd.arg("--fs-exec-and-read").arg(path);
            }
            Exception::Read(path) => {
                cmd.arg("--fs-read").arg(path);
            }
            Exception::ReadAndWrite(path) => {
                cmd.arg("--fs-write-and-read").arg(path);
            }
            Exception::Networking => {
                cmd.arg("--network");
            }
        }
    }

    cmd.arg(exe);

    cmd
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    #[test]
    fn test_sandboxed_command() {
        let cmd = sandboxed_command(
            "test",
            &[
                Exception::ExecuteAndRead("/bin".into()),
                Exception::Read("/etc".into()),
                Exception::ReadAndWrite("/tmp".into()),
                Exception::Networking,
            ],
        );

        let args = cmd.get_args();

        // args to string to compare
        let args: Vec<&OsStr> = args.collect();

        assert_eq!(
            args,
            vec![
                "__sandbox_trampoline__",
                "--fs-exec-and-read",
                "/bin",
                "--fs-read",
                "/etc",
                "--fs-write-and-read",
                "/tmp",
                "--network",
                "test",
            ]
        );
    }
}
