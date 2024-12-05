//! We expose some sandbox items that then call itself through the trampoline
use crate::sandbox::Exception;
use tokio::process::Command;

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
