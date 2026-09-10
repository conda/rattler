use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use rattler_conda_lock::{Document, Manager};
use rattler_lock::conda_lock::{ExportOptions, ImportOptions, export, import, import_document};

fn fixture(name: &str) -> Document {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data/cep37")
        .join(name);
    Document::from_path(path).unwrap()
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
                        .map(|c| (*c).to_owned())
                        .collect::<BTreeSet<_>>(),
                )
            })
            .collect(),
    }
}

#[test]
fn configured_categories_select_exactly_the_requested_packages() {
    let document = fixture("multiple-categories.yml");
    let cep = document.lock_file();
    let pixi = import(
        cep,
        &options(&[
            ("default", &["main"]),
            ("test", &["main", "test"]),
            ("all", &["main", "dev", "test"]),
        ]),
    )
    .unwrap();

    let names = |environment: &str| {
        let environment = pixi.environment(environment).unwrap();
        let platform = environment.platforms().next().unwrap();
        environment
            .packages(platform)
            .unwrap()
            .map(|package| package.name().to_string())
            .collect::<BTreeSet<_>>()
    };
    let expected = |categories: &[&str]| {
        cep.package
            .iter()
            .filter(|package| categories.contains(&package.category.as_str()))
            .map(|package| package.name.clone())
            .collect::<BTreeSet<_>>()
    };

    assert_eq!(names("default"), expected(&["main"]));
    assert_eq!(names("test"), expected(&["main", "test"]));
    // 14 identities appear in both dev and test; selecting both must not duplicate them.
    assert_eq!(names("all"), expected(&["main", "dev", "test"]));
    assert!(names("default").len() < names("test").len());
    assert!(names("test").len() < names("all").len());
}

#[test]
fn exported_environment_reproduces_the_imported_conda_and_pip_packages() {
    let document = fixture("pip-deps.yml");
    let original = document.lock_file();
    let pixi = import_document(&document, &ImportOptions::default()).unwrap();
    let exported = export(
        pixi.default_environment().unwrap(),
        &ExportOptions {
            sources: original.metadata.sources.clone(),
            content_hash: Some(original.metadata.content_hash.clone()),
        },
    )
    .unwrap();

    assert_eq!(exported.metadata.platforms, {
        let mut platforms = original.metadata.platforms.clone();
        platforms.sort();
        platforms
    });
    assert_eq!(
        exported.metadata.content_hash,
        original.metadata.content_hash
    );
    assert_eq!(exported.metadata.channels, original.metadata.channels);

    let identity = |lock: &rattler_conda_lock::LockFile| {
        lock.package
            .iter()
            .map(|package| {
                (
                    package.manager,
                    package.name.to_lowercase(),
                    package.platform.clone(),
                    package.version.clone(),
                    package.url.clone(),
                    package.hash.clone(),
                )
            })
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(identity(&exported), identity(original));
    assert!(
        exported
            .package
            .iter()
            .any(|package| package.manager == Manager::Pip)
    );
    assert!(
        exported
            .package
            .iter()
            .all(|package| package.category == "main" && !package.optional)
    );
    exported.to_yaml().unwrap();
}

#[test]
fn export_generates_deterministic_hashes_when_the_caller_supplies_none() {
    let pixi = import_document(&fixture("blas-mkl.yml"), &ImportOptions::default()).unwrap();
    let environment = pixi.default_environment().unwrap();
    let exported = export(environment, &ExportOptions::default()).unwrap();
    assert_eq!(
        exported.metadata.content_hash,
        export(environment, &ExportOptions::default())
            .unwrap()
            .metadata
            .content_hash
    );
    assert_eq!(
        exported
            .metadata
            .content_hash
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        exported.metadata.platforms
    );
    assert!(
        exported
            .metadata
            .content_hash
            .values()
            .all(|hash| hash.len() == 64)
    );
    // Generated hashes describe packages, so they differ from conda-lock's input hashes.
    assert_ne!(
        exported.metadata.content_hash,
        fixture("blas-mkl.yml").lock_file().metadata.content_hash
    );
}

/// A CEP document with one conda package plus the given extra package entries.
fn document(packages: &str) -> Document {
    Document::parse(&format!(
        "version: 1\nmetadata:\n  content_hash: {{linux-64: '{hash}'}}\n  channels: [{{url: conda-forge, used_env_vars: []}}]\n  platforms: [linux-64]\n  sources: []\npackage:\n- name: python\n  version: '3.12.0'\n  manager: conda\n  platform: linux-64\n  dependencies: {{}}\n  url: https://conda.anaconda.org/conda-forge/linux-64/python-3.12.0-h1234567_0.conda\n  hash: {{sha256: '{hash}'}}\n  optional: false\n{packages}",
        hash = "a".repeat(64),
    ))
    .unwrap()
}

#[test]
fn direct_source_packages_report_their_original_location() {
    let git =
        "git+https://github.com/requests/requests.git@3f07f990ac74f1e1691ba17ef8c14007f05846ab";
    let document = document(&format!(
        "- name: requests\n  version: '2.32.4'\n  manager: pip\n  platform: linux-64\n  dependencies: {{}}\n  url: {git}\n  hash: {{sha256: 3f07f990ac74f1e1691ba17ef8c14007f05846ab}}\n  source: {{type: url, url: {git}}}\n  optional: false\n"
    ));
    let error = import_document(&document, &ImportOptions::default()).unwrap_err();
    assert_eq!(error.path(), "package[1].source");
    let span = error
        .labels()
        .iter()
        .find_map(rattler_conda_lock::Label::span)
        .unwrap();
    let source = document.source_text();
    let line = source[..span.start]
        .rsplit_once('\n')
        .map_or(source, |(_, line)| line);
    assert!(
        line.contains("source:") && source[span.start..].starts_with(['{', 't']),
        "{line:?}"
    );
}

#[test]
fn platform_specific_python_requirements_are_rejected_rather_than_merged() {
    // pixi stores one requirement set per artifact; conda-lock stores one per
    // platform, so `click` requiring `colorama` only on Windows cannot be kept.
    let error =
        import_document(&fixture("upgrade-v3.0.3.yml"), &ImportOptions::default()).unwrap_err();
    assert!(
        error.message().contains("conflicting package metadata"),
        "{}",
        error.message()
    );
    assert_eq!(error.labels().len(), 2);
    assert!(error.labels().iter().all(|label| label.span().is_some()));
}

#[test]
fn unknown_categories_and_empty_selections_are_rejected() {
    let cep = fixture("multiple-categories.yml").into_lock_file();
    for invalid in [
        options(&[("default", &["docs"])]),
        options(&[("default", &[])]),
        ImportOptions {
            environments: BTreeMap::new(),
        },
    ] {
        let error = import(&cep, &invalid).unwrap_err();
        assert!(
            error.path().starts_with("options.environments"),
            "{}",
            error.path()
        );
    }
}

#[test]
fn conda_lock_constraint_spellings_convert_or_fail_explicitly() {
    // conda-lock writes an exact pin as a bare version literal and `*` for an
    // unconstrained dependency.
    let pinned = document(
        "- name: pydantic\n  version: '2.5.1'\n  manager: pip\n  platform: linux-64\n  dependencies: {pydantic-core: '2.14.3', annotated-types: '*', typing-extensions: '>=4.6.1'}\n  url: https://files.pythonhosted.org/packages/ab/pydantic-2.5.1-py3-none-any.whl\n  hash: {sha256: 'dc5244a8939e0d9a68f1f1b5f550b2e1c879912033b1becbedb315accc75441b'}\n  optional: false\n",
    );
    let pixi = import_document(&pinned, &ImportOptions::default()).unwrap();
    let environment = pixi.default_environment().unwrap();
    let platform = environment.platforms().next().unwrap();
    let pydantic = environment
        .packages(platform)
        .unwrap()
        .find(|package| package.name() == "pydantic")
        .unwrap();
    let mut requirements = pydantic
        .as_pypi()
        .unwrap()
        .requires_dist()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    requirements.sort();
    assert_eq!(
        requirements,
        [
            "annotated-types",
            "pydantic-core==2.14.3",
            "typing-extensions>=4.6.1"
        ]
    );

    // Poetry-style alternatives have no PEP 508 equivalent.
    let alternation = document(
        "- name: pydantic-core\n  version: '2.14.3'\n  manager: pip\n  platform: linux-64\n  dependencies: {typing-extensions: '>=4.6.0,<4.7.0 || >4.7.0'}\n  url: https://files.pythonhosted.org/packages/cd/pydantic_core-2.14.3-cp312-none-any.whl\n  hash: {sha256: 'b2b0e0ec4ba0169fb1a7e9ad7c8a2a4b0a1b5d5b1c1f2a7b1a1c1d1e1f101112'}\n  optional: false\n",
    );
    let error = import_document(&alternation, &ImportOptions::default()).unwrap_err();
    assert_eq!(error.path(), "package[1].dependencies.typing-extensions");
    assert!(
        error.message().contains("alternative version constraints"),
        "{}",
        error.message()
    );
}
