use std::path::PathBuf;

use rattler_conda_lock::{Document, LockFile};
use rstest::rstest;
use serde_json::{Value, json};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data/cep37")
        .join(name)
}

fn remove_nulls(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        object.retain(|_, value| !value.is_null());
    }
}

fn lowercase_hashes(value: &mut Value) {
    remove_nulls(value);
    if let Some(object) = value.as_object_mut() {
        for hash in object.values_mut() {
            if let Some(text) = hash.as_str() {
                *hash = Value::String(text.to_ascii_lowercase());
            }
        }
    }
}

fn semantic_yaml(source: &str) -> Value {
    let mut value: Value = serde_yaml::from_str(source).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .entry("version")
        .or_insert(json!(1));
    let metadata = value["metadata"].as_object_mut().unwrap();
    metadata.retain(|_, value| !value.is_null());
    for channel in metadata["channels"].as_array_mut().unwrap() {
        if let Some(url) = channel.as_str() {
            *channel = json!({"url": url, "used_env_vars": []});
        }
    }
    metadata["platforms"]
        .as_array_mut()
        .unwrap()
        .sort_by_key(Value::to_string);
    lowercase_hashes(metadata.get_mut("content_hash").unwrap());
    if let Some(git) = metadata.get_mut("git_metadata") {
        remove_nulls(git);
    }
    if let Some(inputs) = metadata.get_mut("inputs_metadata") {
        for hashes in inputs.as_object_mut().unwrap().values_mut() {
            lowercase_hashes(hashes);
        }
    }
    let packages = value["package"].as_array_mut().unwrap();
    for package in packages.iter_mut() {
        remove_nulls(package);
        let object = package.as_object_mut().unwrap();
        object.entry("category").or_insert(json!("main"));
        object.entry("dependencies").or_insert(json!({}));
        if !object["version"].is_string() {
            object.insert(
                "version".into(),
                Value::String(object["version"].to_string()),
            );
        }
        let is_pip = object["manager"] == "pip";
        for constraint in object
            .get_mut("dependencies")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            if !constraint.is_string() {
                *constraint = Value::String(constraint.to_string());
            }
            if is_pip && constraint == "*" {
                *constraint = json!("");
            }
        }
        lowercase_hashes(object.get_mut("hash").unwrap());
    }
    packages.sort_by_key(|p| {
        ["manager", "name", "platform", "category"]
            .map(|field| p[field].as_str().unwrap().to_owned())
    });
    value
}

#[rstest]
#[case("cep-example.yml")]
#[case("multiple-categories.yml")]
#[case("pip-deps.yml")]
#[case("legacy-lockfile.yml")]
#[case("upgrade-v2.5.8.yml")]
#[case("upgrade-v3.0.2.yml")]
#[case("upgrade-v3.0.3.yml")]
#[case("blas-mkl.yml")]
#[case("explicit-toposorted.yml")]
fn upstream_documents_preserve_semantics(#[case] name: &str) {
    let source = std::fs::read_to_string(fixture_path(name)).unwrap();
    let parsed = Document::parse(&source).unwrap_or_else(|error| panic!("{name}: {error}"));
    let written = parsed.lock_file().to_yaml().unwrap();
    assert_eq!(semantic_yaml(&source), semantic_yaml(&written), "{name}");
    let reparsed: LockFile = written.parse().unwrap();
    assert_eq!(
        written,
        reparsed.to_yaml().unwrap(),
        "unstable output: {name}"
    );
}

#[test]
fn sha256_only_packages_and_empty_hashes_remain_representable() {
    let mut lock = Document::from_path(fixture_path("cep-example.yml"))
        .unwrap()
        .into_lock_file();
    for package in &mut lock.package {
        package.hash.md5 = None;
    }
    lock.package[0].hash.sha256 = None;
    let written = lock.to_yaml().unwrap();
    let reparsed: LockFile = written.parse().unwrap();
    for package in &reparsed.package {
        let original = lock
            .package
            .iter()
            .find(|p| p.platform == package.platform)
            .unwrap();
        assert_eq!(package.hash, original.hash);
    }
    assert!(!written.contains("md5:"));
}

#[test]
fn all_provenance_fields_survive_read_write() {
    let source = format!(
        "version: 1\nmetadata:\n  content_hash: {{linux-64: '{}' }}\n  channels:\n  - url: https://example.org/${{CHANNEL_TOKEN}}\n    used_env_vars: [CHANNEL_TOKEN]\n  platforms: [linux-64]\n  sources: [environment.yml]\n  time_metadata: {{created_at: '2026-09-09T12:00:00Z'}}\n  git_metadata:\n    git_user_name: A Developer\n    git_user_email: developer@example.org\n    git_sha: abc123\n  inputs_metadata:\n    environment.yml:\n      md5: '{}'\n      sha256: '{}'\n  custom_metadata: {{purpose: compatibility}}\npackage: []\n",
        "a".repeat(64),
        "b".repeat(32),
        "c".repeat(64)
    );
    let lock: LockFile = source.parse().unwrap();
    assert_eq!(
        semantic_yaml(&source),
        semantic_yaml(&lock.to_yaml().unwrap())
    );
}

#[test]
fn generated_hashes_ignore_package_order_but_track_artifact_changes() {
    let mut lock = Document::from_path(fixture_path("multiple-categories.yml"))
        .unwrap()
        .into_lock_file();
    let hashes = lock.compute_content_hashes();
    lock.package.reverse();
    assert_eq!(hashes, lock.compute_content_hashes());
    lock.package[0].url.push_str("?different-artifact");
    assert_ne!(hashes, lock.compute_content_hashes());
}

/// The documented preimage is what `json.dumps(packages, sort_keys=True)`
/// produces, so a Python implementation can reproduce these hashes.
#[test]
fn generated_hashes_match_python_json_dumps() {
    let mut lock = LockFile::default();
    lock.metadata.platforms.push("linux-64".into());
    lock.package.push(rattler_conda_lock::Package {
        name: "ca-certificates".into(),
        version: "2025.10.5".into(),
        manager: rattler_conda_lock::Manager::Conda,
        platform: "linux-64".into(),
        dependencies: [("__unix".to_string(), String::new())].into_iter().collect(),
        url: "https://conda.anaconda.org/conda-forge/noarch/ca-certificates-2025.10.5-hbd8a1cb_0.conda".into(),
        hash: rattler_conda_lock::Hashes {
            md5: Some("F9E5FBC24009179E8B0409624691758A".into()),
            sha256: Some("3b5ad78b8bb61b6cdc0978a6a99f8dfb2cc789a451378d054698441005ecbdb6".into()),
        },
        source: None,
        build: Some("hbd8a1cb_0".into()),
        category: "main".into(),
        optional: false,
    });

    let expected_preimage = concat!(
        r#"[{"build": "hbd8a1cb_0", "category": "main", "dependencies": {"__unix": ""}, "#,
        r#""hash": {"md5": "f9e5fbc24009179e8b0409624691758a", "#,
        r#""sha256": "3b5ad78b8bb61b6cdc0978a6a99f8dfb2cc789a451378d054698441005ecbdb6"}, "#,
        r#""manager": "conda", "name": "ca-certificates", "optional": false, "platform": "linux-64", "#,
        r#""url": "https://conda.anaconda.org/conda-forge/noarch/ca-certificates-2025.10.5-hbd8a1cb_0.conda", "#,
        r#""version": "2025.10.5"}]"#,
    );
    let expected = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(expected_preimage));
    assert_eq!(lock.compute_content_hashes()["linux-64"], expected);
}

#[test]
fn validation_errors_retain_utf8_crlf_source_ranges() {
    let source = format!(
        "# café\r\nmetadata:\r\n  content_hash: {{linux-64: '{}'}}\r\n  channels: []\r\n  platforms: [linux-64]\r\n  sources: []\r\npackage:\r\n- name: numpy\r\n  version: '1.0'\r\n  manager: conda\r\n  platform: linux-64\r\n  url: https://example.org/linux-64/numpy-1.0-0.conda\r\n  hash: {{sha256: bad-digest}}\r\n  optional: false\r\n",
        "a".repeat(64)
    );
    let error = Document::parse(&source).unwrap_err();
    assert_eq!(error.path(), "package[0].hash.sha256");
    let span = error
        .labels()
        .iter()
        .find_map(rattler_conda_lock::Label::span)
        .unwrap();
    assert_eq!(&source[span], "bad-digest");
    assert_eq!(error.source_text(), Some(source.as_str()));
}
