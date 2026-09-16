//! Snapshots of every diagnostic this crate can raise while reading a document.
//!
//! These pin the pairing of [`ErrorKind`], model path and source span, so a
//! refactor that moves a span to the wrong node, or drops the second label of a
//! duplicate, shows up as a snapshot diff rather than as a silently worse error.

use std::fmt::Write as _;

use rattler_conda_lock::Document;

/// Renders a diagnostic with everything a caller can act on.
fn report(case: &str, source: &str) -> String {
    let mut out = String::new();
    writeln!(out, "# {case}").unwrap();
    let Err(error) = Document::parse(source) else {
        writeln!(out, "parsed without error").unwrap();
        return out;
    };
    writeln!(out, "kind:    {:?}", error.kind()).unwrap();
    writeln!(out, "display: {error}").unwrap();
    for label in error.labels() {
        let excerpt = label.span().map_or("<no span>".to_owned(), |span| {
            format!("{:?}", &source[span])
        });
        let message = label.message().unwrap_or("<primary>");
        writeln!(
            out,
            "label:   {} [{message}] {excerpt}",
            if label.path().is_root() {
                "<document>"
            } else {
                label.path().as_str()
            }
        )
        .unwrap();
    }
    out
}

/// A valid document with `body` substituted for the package list.
fn with_packages(body: &str) -> String {
    format!(
        "version: 1
metadata:
  content_hash:
    linux-64: {hash}
  channels:
    - url: conda-forge
      used_env_vars: []
  platforms: [linux-64]
  sources: []
package:
{body}",
        hash = "a".repeat(64)
    )
}

/// A valid document with `body` substituted for the whole metadata mapping.
fn with_metadata(body: &str) -> String {
    format!("version: 1\nmetadata:\n{body}package: []\n")
}

const PACKAGE: &str = "  - name: python
    version: '3.13.1'
    manager: conda
    platform: linux-64
    url: https://conda.anaconda.org/conda-forge/linux-64/python-3.13.1-h9e4cc4f_0.conda
    hash:
      sha256: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
    optional: false
";

#[test]
fn syntax_and_schema_errors() {
    let mut report_all = String::new();
    for (case, source) in [
        ("not yaml", "metadata: [\n".to_owned()),
        (
            "two documents",
            format!(
                "{}---\n{}",
                with_packages("  []\n"),
                with_packages("  []\n")
            ),
        ),
        (
            "duplicate mapping key",
            with_metadata(
                "  content_hash: {}\n  channels: []\n  platforms: []\n  sources: []\n  sources: []\n",
            ),
        ),
        (
            "missing field",
            with_metadata("  content_hash: {}\n  channels: []\n  platforms: []\n"),
        ),
        (
            "unknown field",
            with_metadata(
                "  content_hash: {}\n  channels: []\n  platforms: []\n  sources: []\n  future: true\n",
            ),
        ),
        (
            "unsupported version",
            with_packages("  []\n").replace("version: 1", "version: 2"),
        ),
        ("expected a mapping", with_packages("  - just-a-string\n")),
        (
            "expected a sequence",
            with_metadata("  content_hash: {}\n  channels: {}\n  platforms: []\n  sources: []\n"),
        ),
        (
            "expected a string",
            with_metadata(
                "  content_hash: {}\n  channels: []\n  platforms: [[nested]]\n  sources: []\n",
            ),
        ),
        (
            "expected a boolean",
            with_packages(&PACKAGE.replace("optional: false", "optional: maybe")),
        ),
        (
            "unknown manager",
            with_packages(&PACKAGE.replace("manager: conda", "manager: cargo")),
        ),
        (
            "unsupported source type",
            with_packages(&format!(
                "{PACKAGE}    source:\n      type: git\n      url: https://example.org/repo.git\n"
            )),
        ),
    ] {
        report_all.push_str(&report(case, &source));
        report_all.push('\n');
    }
    insta::assert_snapshot!(report_all);
}

#[test]
fn metadata_validation_errors() {
    let mut report_all = String::new();
    for (case, source) in [
        (
            "invalid digest",
            with_metadata(
                "  content_hash: {linux-64: nope}\n  channels: []\n  platforms: [linux-64]\n  sources: []\n",
            ),
        ),
        (
            "duplicate platform",
            with_packages("  []\n").replace("[linux-64]", "[linux-64, linux-64]"),
        ),
        (
            "missing content hash",
            with_metadata(
                "  content_hash: {}\n  channels: []\n  platforms: [linux-64]\n  sources: []\n",
            ),
        ),
        (
            "content hash for undeclared platform",
            with_packages("  []\n").replace(
                &format!("linux-64: {}", "a".repeat(64)),
                &format!("linux-64: {a}\n    osx-64: {a}", a = "a".repeat(64)),
            ),
        ),
        (
            "invalid platform",
            with_packages("  []\n").replace("platforms: [linux-64]", "platforms: [noarch]"),
        ),
        (
            "empty channel",
            with_packages("  []\n").replace("url: conda-forge", "url: '  '"),
        ),
        (
            "absolute source path",
            with_packages("  []\n").replace("sources: []", "sources: ['/etc/environment.yml']"),
        ),
        (
            "duplicate source path",
            with_packages("  []\n").replace("sources: []", "sources: [env.yml, env.yml]"),
        ),
        (
            "invalid timestamp",
            with_packages("  []\n").replace(
                "sources: []",
                "sources: []\n  time_metadata: {created_at: yesterday}",
            ),
        ),
        (
            "input hashes for undeclared source",
            with_packages("  []\n").replace(
                "sources: []",
                "sources: []\n  inputs_metadata:\n    env.yml: {md5: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}",
            ),
        ),
        (
            "missing input hashes",
            with_packages("  []\n").replace(
                "sources: []",
                "sources: [env.yml, other.yml]\n  inputs_metadata:\n    env.yml: {md5: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}",
            ),
        ),
    ] {
        report_all.push_str(&report(case, &source));
        report_all.push('\n');
    }
    insta::assert_snapshot!(report_all);
}

#[test]
fn package_validation_errors() {
    let mut report_all = String::new();
    for (case, source) in [
        (
            "invalid package name",
            with_packages(&PACKAGE.replace("name: python", "name: 'py thon'")),
        ),
        (
            "invalid version",
            with_packages(&PACKAGE.replace("version: '3.13.1'", "version: 'not a version'")),
        ),
        (
            "undeclared package platform",
            with_packages(&PACKAGE.replace("platform: linux-64", "platform: osx-64")),
        ),
        (
            "empty category",
            with_packages(&format!("{PACKAGE}    category: ''\n")),
        ),
        (
            "duplicate package identity",
            with_packages(&format!("{PACKAGE}{PACKAGE}")),
        ),
        (
            "invalid url",
            with_packages(&PACKAGE.replace(
                "url: https://conda.anaconda.org/conda-forge/linux-64/python-3.13.1-h9e4cc4f_0.conda",
                "url: ./python.conda",
            )),
        ),
        (
            "invalid build string",
            with_packages(&format!("{PACKAGE}    build: 'h9e4cc4f 0'\n")),
        ),
        (
            "invalid dependency constraint",
            with_packages(&format!(
                "{PACKAGE}    dependencies:\n      libgcc: '>= <'\n"
            )),
        ),
        (
            "invalid revision on a source package",
            with_packages(
                &format!(
                    "{PACKAGE}    source:\n      type: url\n      url: https://example.org/repo.git\n"
                )
                .replace(&"b".repeat(64), "not-a-revision"),
            ),
        ),
    ] {
        report_all.push_str(&report(case, &source));
        report_all.push('\n');
    }
    insta::assert_snapshot!(report_all);
}
