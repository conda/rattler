//! End-to-end tests that run the compiled `rattler` binary against local test
//! packages and snapshot its output, so unintended changes to command output
//! show up as a snapshot diff in review.

use std::process::Command;

const EMPTY_PACKAGE: &str = "test-data/packages/empty-0.1.0-h4616a5c_0.conda";
const CLOBBER_PACKAGE: &str = "test-data/clobber/clobber-1-0.2.0-h4616a5c_0.tar.bz2";

/// Runs the `rattler` binary from the workspace root (so the test packages can
/// be addressed with stable relative paths) and returns its stdout. Styling is
/// disabled automatically because stdout is not a terminal.
fn run_rattler(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_rattler"))
        .args(args)
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .output()
        .expect("failed to run the rattler binary");
    assert!(
        output.status.success(),
        "rattler {args:?} failed with {}:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("rattler wrote non-utf8 output")
}

#[test]
fn test_inspect_local_package() {
    insta::assert_snapshot!(run_rattler(&["inspect", EMPTY_PACKAGE]));
}

#[test]
fn test_inspect_local_package_json() {
    insta::assert_snapshot!(run_rattler(&["inspect", "--json", EMPTY_PACKAGE]));
}

#[test]
fn test_compare_identical_packages() {
    insta::assert_snapshot!(run_rattler(&[
        "compare-packages",
        EMPTY_PACKAGE,
        EMPTY_PACKAGE
    ]));
}

#[test]
fn test_compare_different_packages() {
    insta::assert_snapshot!(run_rattler(&[
        "compare-packages",
        EMPTY_PACKAGE,
        CLOBBER_PACKAGE
    ]));
}
