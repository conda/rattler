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

/// `rattler list` must report every record it can read and warn about the
/// ones it cannot, instead of silently producing an incomplete listing.
#[test]
fn test_list_warns_about_unreadable_records() {
    use std::str::FromStr;

    use rattler_conda_types::{PackageName, PackageRecord, PrefixRecord, RepoDataRecord, Version};

    let prefix = tempfile::tempdir().unwrap();
    let meta_dir = prefix.path().join("conda-meta");
    std::fs::create_dir(&meta_dir).unwrap();

    let record = PrefixRecord::from_repodata_record(
        RepoDataRecord {
            package_record: PackageRecord::new(
                PackageName::new_unchecked("my-package"),
                Version::from_str("0.0.1").unwrap(),
                "h123456_0".to_string(),
            ),
            identifier: "my-package-0.0.1-h123456_0.conda".parse().unwrap(),
            url: url::Url::parse("https://example.com/my-package-0.0.1-h123456_0.conda").unwrap(),
            channel: None,
        },
        Vec::new(),
    );
    record
        .write_to_path(meta_dir.join("my-package-0.0.1-h123456_0.json"), true)
        .unwrap();
    std::fs::write(meta_dir.join("broken-1.0-0.json"), "{ this is not json").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_rattler"))
        .args(["list", "-p", prefix.path().to_str().unwrap()])
        .output()
        .expect("failed to run the rattler binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "rattler list failed:\n{stderr}");
    assert!(stdout.contains("my-package"), "stdout was:\n{stdout}");
    assert!(!stdout.contains("broken"), "stdout was:\n{stdout}");
    assert!(
        stderr.contains("could not be read"),
        "stderr was:\n{stderr}"
    );
}
