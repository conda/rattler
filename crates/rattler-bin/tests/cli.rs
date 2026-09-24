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
    run_rattler_with_env(args, &[])
}

/// [`run_rattler`] with extra environment variables set for the run.
fn run_rattler_with_env(args: &[&str], env: &[(&str, &str)]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_rattler"))
        .args(args)
        .envs(env.iter().copied())
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

/// The start of an OSC 8 hyperlink.
const OSC8: &str = "\u{1b}]8;;";

/// Whatever else the output carries, a consumer that cannot render hyperlinks
/// must not receive any. Since stdout is a pipe here, this is the default for
/// every command.
#[test]
fn test_no_hyperlinks_when_stdout_is_not_a_terminal() {
    for args in [
        vec!["list", "-p", FIXTURE_PREFIX],
        vec!["list", "-p", FIXTURE_PREFIX, "--format", "urls"],
        vec!["inspect", EMPTY_PACKAGE],
        vec!["info"],
    ] {
        let output = run_rattler(&args);
        assert!(
            !output.contains(OSC8),
            "rattler {args:?} wrote a hyperlink to a piped stdout"
        );
    }
}

/// `FORCE_HYPERLINK` opts a non-terminal stdout in, which is also the only way
/// to observe the links in a test.
#[test]
fn test_hyperlinks_can_be_forced() {
    let output = run_rattler_with_env(
        &["list", "-p", FIXTURE_PREFIX, "--full-name", "bzip2"],
        &[("FORCE_HYPERLINK", "1")],
    );
    // The package name links to its page on prefix.dev, which mirrors the
    // conda-forge channel the fixture records come from, and the name itself is
    // still there in full.
    assert!(
        output.contains(&format!(
            "{OSC8}https://prefix.dev/channels/conda-forge/packages/bzip2\u{1b}\\bzip2\u{1b}]8;;\u{1b}\\"
        )),
        "expected a hyperlinked package name, got:\n{output}"
    );
    // Machine-readable output stays plain even then.
    let urls = run_rattler_with_env(
        &["list", "-p", FIXTURE_PREFIX, "--format", "urls"],
        &[("FORCE_HYPERLINK", "1")],
    );
    assert!(!urls.contains(OSC8), "got hyperlinks in --format urls");
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
