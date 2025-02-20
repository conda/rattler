use assert_cmd::cargo::CommandCargoExt;
use std::process::Command;

#[tokio::test]
async fn test_cli() {
    let mut cmd = Command::cargo_bin("rattler-index").unwrap();
    let args = ["--verbose", "fs", "."];

    let output = cmd.args(args).output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    println!("{stdout}");
    println!("{stderr}");
}
