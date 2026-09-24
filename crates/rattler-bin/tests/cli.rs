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

/// Snapshots of `rattler verify-attestation` for a real signed package.
#[cfg(feature = "sigstore")]
mod attestation {
    use super::run_rattler;

    /// A package published to <https://prefix.dev/skill-forge> by a GitHub
    /// Actions workflow, and the attestation sidecar it was published with. Both
    /// were downloaded from `https://prefix.dev/skill-forge/noarch/` and are
    /// kept verbatim so the snapshots describe a real attestation. See
    /// `test-data/sigstore/README.md`.
    const PACKAGE: &str = "test-data/sigstore/agent-skill-conda-forge-0.0.21-h4616a5c_0.conda";
    const SIDECAR: &str = "test-data/sigstore/agent-skill-conda-forge-0.0.21-h4616a5c_0.conda.sigs";

    /// The channel the sidecar's `targetChannel` names, which the verification
    /// compares the channel of the package against.
    const CHANNEL: &str = "https://prefix.dev/skill-forge";

    /// Verifies the fixture the way the published package is verified, but
    /// entirely from local files: `--offline` keeps both the sidecar retrieval
    /// and the trusted root off the network, so the test does not depend on
    /// prefix.dev or on the Sigstore TUF repository being reachable.
    ///
    /// The signing certificate is checked against the time the signature was
    /// recorded in the transparency log rather than against the current time, so
    /// the fixture does not expire together with the certificate.
    fn run_verify_attestation(extra_args: &[&str]) -> String {
        let mut args = vec![
            "--offline",
            "verify-attestation",
            "--attestation",
            SIDECAR,
            PACKAGE,
            "--channel",
            CHANNEL,
        ];
        args.extend_from_slice(extra_args);
        normalize_file_urls(&run_rattler(&args))
    }

    /// Replaces the absolute `file://` URL a local sidecar is reported under, so
    /// the snapshots do not depend on where the repository was checked out.
    fn normalize_file_urls(output: &str) -> String {
        regex::Regex::new(r"file://\S*/test-data/")
            .expect("the pattern is valid")
            .replace_all(output, "file:///[ROOT]/test-data/")
            .into_owned()
    }

    #[test]
    fn test_verify_attestation() {
        insta::assert_snapshot!(run_verify_attestation(&[]));
    }

    /// The JSON output carries every claim of the signing certificate and the
    /// full transparency log metadata, including what the human output folds
    /// together or leaves out.
    #[test]
    fn test_verify_attestation_json() {
        insta::assert_snapshot!(run_verify_attestation(&["--format", "json"]));
    }
}
