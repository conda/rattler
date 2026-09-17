//! Snapshots of the conversion diagnostics, rendered with miette so the CEP-37
//! source spans they resolve to are visible. They pin which install-changing
//! situations are refused and where the refusal points.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use miette::{GraphicalReportHandler, GraphicalTheme};
use rattler_conda_lock::Document;
use rattler_lock::LockFile;
use rattler_lock::conda_lock::ImportOptions;

const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// A document holding one valid conda package plus `extra` package entries.
fn document(extra: &str) -> String {
    format!(
        "version: 1
metadata:
  content_hash:
    linux-64: {DIGEST}
  channels:
    - url: conda-forge
      used_env_vars: []
  platforms: [linux-64]
  sources: []
package:
  - name: python
    version: '3.13.1'
    manager: conda
    platform: linux-64
    url: https://conda.anaconda.org/conda-forge/linux-64/python-3.13.1-h9e4cc4f_0.conda
    hash:
      sha256: {DIGEST}
    optional: false
{extra}"
    )
}

fn options(environments: &[(&str, &[&str])]) -> ImportOptions {
    ImportOptions {
        environments: environments
            .iter()
            .map(|(name, categories)| {
                (
                    (*name).to_owned(),
                    categories
                        .iter()
                        .map(|category| (*category).to_owned())
                        .collect::<BTreeSet<_>>(),
                )
            })
            .collect(),
    }
}

fn report(case: &str, source: &str, options: &ImportOptions) -> String {
    let mut out = String::new();
    writeln!(out, "# {case}").unwrap();
    let document = Document::parse(source).expect("the document itself is valid CEP-37");
    let Err(error) = LockFile::from_conda_lock_document(&document, options) else {
        writeln!(out, "imported without error").unwrap();
        return out;
    };
    writeln!(out, "kind: {:?}", error.kind()).unwrap();
    GraphicalReportHandler::new_themed(GraphicalTheme::unicode_nocolor())
        .with_width(100)
        .render_report(&mut out, &error.report())
        .unwrap();
    out
}

#[test]
fn artifacts_that_cannot_be_installed_as_written() {
    let mut all = String::new();
    for (case, extra) in [
        (
            "package built from a source location",
            "  - name: requests
    version: '2.32.4'
    manager: pip
    platform: linux-64
    url: https://github.com/psf/requests.git
    hash:
      sha256: 3f07f990ac74f1e1691ba17ef8c14007f05846ab
    source:
      type: url
      url: https://github.com/psf/requests.git
    optional: false
"
            .to_owned(),
        ),
        (
            "url that is not a fetchable artifact",
            format!(
                "  - name: requests
    version: '2.32.4'
    manager: pip
    platform: linux-64
    url: git+https://github.com/psf/requests.git
    hash:
      sha256: {DIGEST}
    optional: false
"
            ),
        ),
        (
            "url that only survives normalization",
            format!(
                "  - name: numpy
    version: '2.1.0'
    manager: conda
    platform: linux-64
    url: https://Conda.Anaconda.org/conda-forge/linux-64/numpy-2.1.0-py312_0.conda
    hash:
      sha256: {DIGEST}
    optional: false
"
            ),
        ),
        (
            "url without an artifact file name",
            format!(
                "  - name: numpy
    version: '2.1.0'
    manager: conda
    platform: linux-64
    url: https://conda.anaconda.org/conda-forge/linux-64/
    hash:
      sha256: {DIGEST}
    optional: false
"
            ),
        ),
        (
            "url without a parsable archive name",
            format!(
                "  - name: numpy
    version: '2.1.0'
    manager: conda
    platform: linux-64
    url: https://conda.anaconda.org/conda-forge/linux-64/numpy.conda
    hash:
      sha256: {DIGEST}
    optional: false
"
            ),
        ),
        (
            "artifact subdir disagrees with the target",
            format!(
                "  - name: numpy
    version: '2.1.0'
    manager: conda
    platform: linux-64
    url: https://conda.anaconda.org/conda-forge/osx-64/numpy-2.1.0-py312_0.conda
    hash:
      sha256: {DIGEST}
    optional: false
"
            ),
        ),
        (
            "conda build string on a python artifact",
            format!(
                "  - name: click
    version: '8.1.7'
    manager: pip
    platform: linux-64
    url: https://files.pythonhosted.org/packages/00/click-8.1.7-py3-none-any.whl
    hash:
      sha256: {DIGEST}
    build: py_0
    optional: false
"
            ),
        ),
    ] {
        all.push_str(&report(case, &document(&extra), &ImportOptions::default()));
        all.push('\n');
    }
    insta::assert_snapshot!(all);
}

#[test]
fn dependencies_that_cannot_be_represented() {
    let mut all = String::new();
    for (case, extra) in [
        (
            "poetry style alternatives",
            format!(
                "  - name: pydantic
    version: '2.5.1'
    manager: pip
    platform: linux-64
    dependencies:
      typing-extensions: '>=4.6.0,<4.7.0 || >4.7.0'
    url: https://files.pythonhosted.org/packages/ab/pydantic-2.5.1-py3-none-any.whl
    hash:
      sha256: {DIGEST}
    optional: false
"
            ),
        ),
        (
            "two dependencies with one normalized name",
            format!(
                "  - name: numpy
    version: '2.1.0'
    manager: conda
    platform: linux-64
    dependencies:
      libgcc: '>=13'
      Libgcc: '>=14'
    url: https://conda.anaconda.org/conda-forge/linux-64/numpy-2.1.0-py312_0.conda
    hash:
      sha256: {DIGEST}
    optional: false
"
            ),
        ),
    ] {
        all.push_str(&report(case, &document(&extra), &ImportOptions::default()));
        all.push('\n');
    }
    insta::assert_snapshot!(all);
}

#[test]
fn selections_that_do_not_describe_one_environment() {
    let categorized = format!(
        "version: 1
metadata:
  content_hash:
    linux-64: {DIGEST}
  channels:
    - url: conda-forge
      used_env_vars: []
  platforms: [linux-64]
  sources: []
package:
  - name: numpy
    version: '2.1.0'
    manager: conda
    platform: linux-64
    url: https://conda.anaconda.org/conda-forge/linux-64/numpy-2.1.0-py312_0.conda
    hash:
      sha256: {DIGEST}
    category: main
    optional: false
  - name: numpy
    version: '2.2.0'
    manager: conda
    platform: linux-64
    url: https://conda.anaconda.org/conda-forge/linux-64/numpy-2.2.0-py312_0.conda
    hash:
      sha256: {DIGEST}
    category: test
    optional: false
  - name: scipy
    version: '1.14.0'
    manager: conda
    platform: linux-64
    url: https://conda.anaconda.org/conda-forge/linux-64/scipy-1.14.0-py312_0.conda
    hash:
      sha256: {DIGEST}
    category: main
    optional: false
  - name: scipy-clone
    version: '1.14.0'
    manager: conda
    platform: linux-64
    url: https://conda.anaconda.org/conda-forge/linux-64/scipy-1.14.0-py312_0.conda
    hash:
      sha256: {DIGEST}
    category: test
    optional: false
"
    );
    let mut all = String::new();
    all.push_str(&report(
        "incompatible packages in two selected categories",
        &categorized,
        &options(&[("default", &["main", "test"])]),
    ));
    all.push('\n');
    all.push_str(&report(
        "one artifact with two package identities",
        &categorized,
        &options(&[("default", &["main"]), ("test", &["test"])]),
    ));
    all.push('\n');
    all.push_str(&report(
        "category that occurs in no package",
        &categorized,
        &options(&[("default", &["docs"])]),
    ));
    all.push('\n');
    all.push_str(&report(
        "environment without categories",
        &categorized,
        &options(&[("default", &[])]),
    ));
    all.push('\n');
    all.push_str(&report(
        "no environments at all",
        &categorized,
        &ImportOptions {
            environments: BTreeMap::new(),
        },
    ));
    insta::assert_snapshot!(all);
}
