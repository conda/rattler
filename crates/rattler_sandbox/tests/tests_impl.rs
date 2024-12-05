#![cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64"),
))]

use libtest_mimic::{Failed, Trial};
use rattler_sandbox::sandboxed_command;

fn test_cannot_ls() -> Result<(), Failed> {
    let mut cmd = sandboxed_command("ls", &[]);
    cmd.arg("/");
    let output = cmd.output().unwrap();
    assert!(!output.status.success());
    Ok(())
}

pub fn tests() -> Vec<Trial> {
    vec![Trial::test("test_cannot_ls", test_cannot_ls)]
}
