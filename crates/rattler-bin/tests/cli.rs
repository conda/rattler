//! End-to-end tests that run the compiled `rattler` binary against local test
//! packages and snapshot its output, so unintended changes to command output
//! show up as a snapshot diff in review.

use std::process::Command;

const EMPTY_PACKAGE: &str = "test-data/packages/empty-0.1.0-h4616a5c_0.conda";
const CLOBBER_PACKAGE: &str = "test-data/clobber/clobber-1-0.2.0-h4616a5c_0.tar.bz2";

/// The test packages are addressed with stable relative paths from here.
const WORKSPACE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

/// Runs the `rattler` binary from the workspace root (so the test packages can
/// be addressed with stable relative paths) and returns its stdout. Styling is
/// disabled automatically because stdout is not a terminal.
fn run_rattler(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_rattler"))
        .args(args)
        .current_dir(WORKSPACE_ROOT)
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

/// `test-data` doubles as a prefix here: its `conda-meta` directory holds a
/// fixed set of prefix records. It carries more than one record for a few
/// package names, of which `PrefixData` keeps the last one by file name, so the
/// listing is the same on every platform.
const FIXTURE_PREFIX: &str = "test-data";

#[test]
fn test_list_urls() {
    insta::assert_snapshot!(run_rattler(&[
        "list",
        "-p",
        FIXTURE_PREFIX,
        "--format",
        "urls"
    ]));
}

#[test]
fn test_list_json() {
    // A single record keeps the snapshot readable; the format is the same for
    // the whole listing.
    insta::assert_snapshot!(run_rattler(&[
        "list",
        "-p",
        FIXTURE_PREFIX,
        "--full-name",
        "bzip2",
        "--format",
        "json"
    ]));
}

/// The skill embeds the crate version, which is replaced so the snapshot does
/// not change on every release.
///
/// The command tree depends on the enabled cargo features (`sigstore` adds
/// `rattler verify-attestation` and the attestation flags), so the snapshot only
/// describes a fully featured build. Configurations that turn features off (the
/// musl CI jobs and `pixi run test` build with `--no-default-features`) skip
/// this test instead of carrying a snapshot per feature combination.
#[cfg(feature = "sigstore")]
#[test]
fn test_skill() {
    let skill = run_rattler(&["skill"]).replace(env!("CARGO_PKG_VERSION"), "[VERSION]");
    insta::assert_snapshot!(skill);
}

/// `lib/blob.bin` in this package is larger than the buffer of a pipe, so the
/// cli cannot finish writing it before the reader on the other end goes away.
const SPARSE_PACKAGE: &str = "test-data/sparse/sparse-test-1.0.0-0.conda";

/// Output of the cli is commonly piped into a program that stops reading before
/// the end (`rattler fetch-file ... | head`). The broken pipe that follows is a
/// normal way for the pipeline to end and should not be reported as an error.
#[test]
fn test_fetch_file_tolerates_closed_stdout() {
    use std::{io::Read, process::Stdio};

    let mut child = Command::new(env!("CARGO_BIN_EXE_rattler"))
        .args(["fetch-file", SPARSE_PACKAGE, "lib/blob.bin"])
        .current_dir(WORKSPACE_ROOT)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to run the rattler binary");

    // Read a bit and then close the pipe while the cli still has data to write.
    let mut stdout = child.stdout.take().expect("stdout was piped");
    let mut head = [0u8; 16];
    stdout
        .read_exact(&mut head)
        .expect("the cli wrote less than 16 bytes");
    drop(stdout);

    let output = child
        .wait_with_output()
        .expect("failed to wait for the rattler binary");
    assert!(
        output.status.success(),
        "a closed stdout should not fail the cli, got {}:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}
