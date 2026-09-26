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

/// Writes a small lookup index (one base layer plus a delta layer) for
/// `linux-64` and `noarch` into a temporary directory, as `rattler-index
/// --write-lookup` would, and returns the directory.
fn write_lookup_index() -> tempfile::TempDir {
    use rattler_lookup::{Kind, Manifest, WriteOptions, write_layer};

    let root = tempfile::tempdir().unwrap();
    let channel = "https://conda.anaconda.org/conda-forge/";
    let subdirs = [
        (
            "linux-64",
            vec![
                (
                    "zlib-1.3.1-hb9d3cd8_2.conda".to_string(),
                    vec![
                        "include/zlib.h".to_string(),
                        "lib/libz.so.1.3.1".to_string(),
                        "lib/libz.so".to_string(),
                    ],
                ),
                (
                    "zlib-1.2.13-h4ab18f5_6.conda".to_string(),
                    vec![
                        "include/zlib.h".to_string(),
                        "lib/libz.so.1.2.13".to_string(),
                    ],
                ),
                (
                    "mariadb-connector-c-3.4.1-h1234_0.tar.bz2".to_string(),
                    vec![
                        "include/zlib.h".to_string(),
                        "lib/libmariadb.so.3".to_string(),
                    ],
                ),
            ],
        ),
        (
            "noarch",
            vec![(
                "polars-1.44.2-pyh3138b34_0.conda".to_string(),
                vec![
                    "site-packages/polars/__init__.py".to_string(),
                    "site-packages/polars/io/__init__.py".to_string(),
                ],
            )],
        ),
    ];
    for (subdir, artifacts) in subdirs {
        let dir = root.path().join(subdir).join("lookup");
        let mut manifest = Manifest::empty(channel, subdir, &Kind::ALL);
        let (base, delta) = artifacts.split_at(artifacts.len() - 1);
        for artifacts in [base, delta] {
            let layer = write_layer(
                &dir,
                channel,
                subdir,
                &Kind::ALL,
                artifacts.to_vec(),
                &WriteOptions::default(),
            )
            .unwrap();
            manifest.layers.push(layer.layer);
        }
        manifest.removed = vec!["zlib-1.2.13-h4ab18f5_6.conda".to_string()];
        std::fs::write(dir.join("manifest.json"), manifest.to_json().unwrap()).unwrap();
        // The channel's repodata points to the manifest, as a channel does.
        std::fs::write(
            root.path().join(subdir).join("repodata.json"),
            serde_json::json!({
                "info": { "subdir": subdir, "lookup_url": "lookup/manifest.json" },
                "packages": {},
                "packages.conda": {},
            })
            .to_string(),
        )
        .unwrap();
    }
    root
}

#[test]
fn test_whoprovides_urls() {
    let index = write_lookup_index();
    insta::assert_snapshot!(run_rattler(&[
        "whoprovides",
        "--channels",
        index.path().to_str().unwrap(),
        "--platform",
        "linux-64",
        "--format",
        "urls",
        "include/zlib.h",
        "**/__init__.py",
        "lib/libz.so*",
    ]));
}

#[test]
fn test_whoprovides_json() {
    let index = write_lookup_index();
    insta::assert_snapshot!(run_rattler(&[
        "whoprovides",
        "--channels",
        index.path().to_str().unwrap(),
        "--platform",
        "linux-64",
        "--format",
        "json",
        "**/zlib.h",
        "does/not/exist",
    ]));
}

#[test]
fn test_whoprovides_human_readable() {
    let index = write_lookup_index();
    let output = run_rattler(&[
        "whoprovides",
        "--channels",
        index.path().to_str().unwrap(),
        "--platform",
        "linux-64",
        "include/zlib.h",
        "**/__init__.py",
        "does/not/exist",
    ]);
    // Timings make the output unsuitable for a snapshot.
    assert!(output.contains("Found 2 packages (2 records) that provide 'include/zlib.h'"));
    assert!(output.contains("  zlib 1.3.1 hb9d3cd8_2 (linux-64)\n"));
    assert!(output.contains("  mariadb-connector-c 3.4.1 h1234_0 (linux-64)\n"));
    assert!(output.contains("Found 1 package (1 record) that provides '**/__init__.py'"));
    assert!(output.contains(
        "  polars 1.44.2 pyh3138b34_0 (noarch) site-packages/polars/__init__.py and 1 more record\n"
    ));
    assert!(output.contains("No packages found that provide 'does/not/exist'"));
}
