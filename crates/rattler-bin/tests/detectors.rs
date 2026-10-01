#![cfg(unix)]

use std::{
    fs::File,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use rattler_conda_types::{
    PrefixRecord, Subdir,
    compression_level::CompressionLevel,
    package::{PathType, PathsEntry, PathsJson},
};
use rattler_digest::{Sha256, compute_bytes_digest};
use rattler_index::{
    ChannelMetadata, IndexFsConfig, PackageRevisionAssignment, index_fs_with_channel_metadata,
};
use rattler_package_streaming::write::write_tar_bz2_package;

struct Harness {
    dir: tempfile::TempDir,
    channel: String,
}

impl Harness {
    async fn new(consent: Option<&str>, detector_body: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let channel = dir.path().join("channel");
        let noarch = channel.join("noarch");
        std::fs::create_dir_all(&noarch).unwrap();
        let staging = dir.path().join("staging");
        write_package(
            &staging,
            &noarch,
            "cli-detect",
            &["__unix >=0"],
            &[],
            &[
                (
                    "bin/cli-detect",
                    detector_body.unwrap_or(
                        "#!/bin/sh\nprintf 'executed\\n' >> \"$CLI_DETECTOR_MARKER\"\necho '{\"version\":1,\"virtual_packages\":{\"__cuda\":{\"version\":\"42\"},\"__cli_capability\":{\"version\":\"7\"}}}'\n",
                    ),
                ),
                (
                    "bin/.cli-detect-post-link.sh",
                    "#!/bin/sh\nprintf 'link-script\\n' >> \"$CLI_DETECTOR_MARKER\"\nexit 1\n",
                ),
            ],
        );
        write_package(
            &staging,
            &noarch,
            "cli-consumer",
            &["__cuda >=42", "__cli_capability >=7"],
            &[],
            &[("bin/cli-consumer", "#!/bin/sh\necho real-consumer\n")],
        );
        write_package(
            &staging,
            &noarch,
            "cli-unrelated",
            &[],
            &[],
            &[("bin/cli-unrelated", "#!/bin/sh\necho unrelated\n")],
        );
        write_package(
            &staging,
            &noarch,
            "cli-constrained",
            &[],
            &["__cli_capability >=8"],
            &[("bin/cli-constrained", "#!/bin/sh\necho constrained\n")],
        );
        index_fs_with_channel_metadata(
            IndexFsConfig {
                channel: channel.clone(),
                target_platform: Some(Subdir::NoArch),
                repodata_patch: None,
                write_zst: false,
                write_shards: false,
                repodata_revisions: Vec::new(),
                package_revision_assignment: PackageRevisionAssignment::FromIndexJson,
                force: true,
                max_parallel: 1,
                multi_progress: None,
            },
            ChannelMetadata {
                virtual_package_detectors: Some(
                    [(
                        "cli-detect".to_string(),
                        vec!["__cuda".to_string(), "__cli_capability".to_string()],
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..ChannelMetadata::default()
            },
        )
        .await
        .unwrap();
        let channel = url::Url::from_directory_path(channel).unwrap().to_string();
        let harness = Self { dir, channel };
        if let Some(consent) = consent {
            let config = harness.dir.path().join("config/config.toml");
            std::fs::create_dir_all(config.parent().unwrap()).unwrap();
            std::fs::write(
                config,
                format!(
                    "[virtual-package-detectors.consent]\n{} = {consent:?}\n",
                    serde_json::to_string(&harness.channel).unwrap(),
                ),
            )
            .unwrap();
        }
        harness
    }

    fn command(&self, command: &str) -> Command {
        self.command_with_format(command, "json")
    }

    fn command_with_format(&self, command: &str, format: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rattler"));
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("CONDA_OVERRIDE_") {
                cmd.env_remove(key);
            }
        }
        cmd.current_dir(self.dir.path())
            .env("HOME", self.dir.path().join("home"))
            .env("XDG_CONFIG_HOME", self.dir.path().join("xdg-config"))
            .env("RATTLER_HOME", self.dir.path().join("config"))
            .env("RATTLER_CACHE_DIR", self.dir.path().join("cache"))
            .env("CLI_DETECTOR_MARKER", self.marker())
            .args(["--offline", command, "-c", &self.channel]);
        if command == "create" {
            cmd.arg("--prefix").arg(self.prefix());
        } else {
            cmd.args(["--format", format]);
        }
        cmd
    }

    fn marker(&self) -> PathBuf {
        self.dir.path().join("detector-executions")
    }

    fn prefix(&self) -> PathBuf {
        self.dir.path().join("prefix")
    }

    fn assert_not_executed(&self) {
        assert!(!self.marker().exists(), "detector or link script executed");
        assert!(!self.prefix().exists(), "consumer prefix was created");
        assert!(
            !self
                .dir
                .path()
                .join("cache/virtual-package-detectors/environments")
                .exists(),
            "detector environment was created",
        );
    }
}

fn write_package(
    staging: &Path,
    subdir: &Path,
    name: &str,
    depends: &[&str],
    constrains: &[&str],
    files: &[(&str, &str)],
) {
    let base = staging.join(name);
    let info = base.join("info");
    std::fs::create_dir_all(&info).unwrap();
    let mut paths = Vec::new();
    let mut archive_paths = Vec::new();
    for (relative, contents) in files {
        let relative = PathBuf::from(relative);
        let path = base.join(&relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        paths.push(PathsEntry {
            relative_path: relative,
            no_link: false,
            path_type: PathType::HardLink,
            prefix_placeholder: None,
            sha256: Some(compute_bytes_digest::<Sha256>(contents.as_bytes())),
            size_in_bytes: Some(contents.len() as u64),
        });
        archive_paths.push(path);
    }
    let index = serde_json::json!({
        "name": name,
        "version": "1.0.0",
        "build": "0",
        "build_number": 0,
        "subdir": "noarch",
        "noarch": "generic",
        "depends": depends,
        "constrains": constrains,
    });
    std::fs::write(info.join("index.json"), serde_json::to_vec(&index).unwrap()).unwrap();
    std::fs::write(
        info.join("paths.json"),
        serde_json::to_vec(&PathsJson {
            paths,
            paths_version: 1,
        })
        .unwrap(),
    )
    .unwrap();
    archive_paths.extend([info.join("index.json"), info.join("paths.json")]);
    write_tar_bz2_package(
        File::create(subdir.join(format!("{name}-1.0.0-0.tar.bz2"))).unwrap(),
        &base,
        &archive_paths,
        CompressionLevel::Default,
        None,
        None,
    )
    .unwrap();
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}

fn assert_consumer_json(output: &Output) {
    assert_success(output);
    let records: Vec<rattler_conda_types::RepoDataRecord> =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].package_record.name.as_normalized(),
        "cli-consumer"
    );
    assert_eq!(records[0].package_record.version.to_string(), "1.0.0");
}

#[tokio::test]
async fn solve_and_create_use_real_channel_detectors_offline() {
    let harness = Harness::new(Some("allow"), None).await;
    let solved = harness
        .command("solve")
        .arg("cli-consumer")
        .output()
        .unwrap();
    assert_consumer_json(&solved);
    assert_eq!(
        std::fs::read_to_string(harness.marker()).unwrap(),
        "executed\n"
    );

    let urls = harness
        .command_with_format("solve", "urls")
        .arg("cli-consumer")
        .output()
        .unwrap();
    assert_success(&urls);
    let url = String::from_utf8(urls.stdout).unwrap();
    assert_eq!(
        url.trim(),
        format!("{}noarch/cli-consumer-1.0.0-0.tar.bz2", harness.channel)
    );

    let installed = harness
        .command("create")
        .arg("cli-consumer")
        .output()
        .unwrap();
    assert_success(&installed);
    let records = PrefixRecord::collect_from_prefix::<PrefixRecord>(&harness.prefix()).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0]
            .repodata_record
            .package_record
            .name
            .as_normalized(),
        "cli-consumer"
    );
    let consumer = Command::new(harness.prefix().join("bin/cli-consumer"))
        .output()
        .unwrap();
    assert_success(&consumer);
    assert_eq!(consumer.stdout, b"real-consumer\n");
    assert!(!harness.prefix().join("bin/cli-detect").exists());
    assert!(
        !std::fs::read_to_string(harness.marker())
            .unwrap()
            .contains("link-script")
    );
}

#[tokio::test]
async fn missing_or_denied_consent_cannot_execute_or_create() {
    for consent in [None, Some("deny")] {
        let harness = Harness::new(consent, None).await;
        for command in ["solve", "create"] {
            let output = harness
                .command(command)
                .arg("cli-consumer")
                .output()
                .unwrap();
            assert!(!output.status.success());
            if command == "solve" {
                assert!(output.stdout.is_empty());
            }
            harness.assert_not_executed();
        }
    }
}

#[tokio::test]
async fn explicit_capabilities_and_environment_overrides_win_without_execution() {
    for consent in [None, Some("deny"), Some("allow")] {
        let harness = Harness::new(consent, None).await;
        let cli = harness
            .command("solve")
            .args([
                "cli-consumer",
                "--virtual-package",
                "__cuda=50",
                "--virtual-package",
                "__cli_capability=8",
            ])
            .output()
            .unwrap();
        assert_consumer_json(&cli);
        let env = harness
            .command("solve")
            .env("CONDA_OVERRIDE_CUDA", "50")
            .env("CONDA_OVERRIDE_CLI_CAPABILITY", "8")
            .arg("cli-consumer")
            .output()
            .unwrap();
        assert_consumer_json(&env);
        harness.assert_not_executed();
        let created = harness
            .command("create")
            .args([
                "cli-consumer",
                "--virtual-package",
                "__cuda=50",
                "--virtual-package",
                "__cli_capability=8",
            ])
            .output()
            .unwrap();
        assert_success(&created);
        let records = PrefixRecord::collect_from_prefix::<PrefixRecord>(&harness.prefix()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0]
                .repodata_record
                .package_record
                .name
                .as_normalized(),
            "cli-consumer"
        );
        assert!(!harness.marker().exists());
        let created_with_env = harness
            .command("create")
            .env("CONDA_OVERRIDE_CUDA", "50")
            .env("CONDA_OVERRIDE_CLI_CAPABILITY", "8")
            .arg("cli-consumer")
            .output()
            .unwrap();
        assert_success(&created_with_env);
        assert!(!harness.marker().exists());
    }
}

#[tokio::test]
async fn foreign_target_never_executes_host_detectors() {
    let harness = Harness::new(Some("allow"), None).await;
    for command in ["solve", "create"] {
        let output = harness
            .command(command)
            .args(["cli-consumer", "--platform", "win-64"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("foreign target"));
        harness.assert_not_executed();
    }
    let output = harness
        .command("solve")
        .args(["cli-consumer", "--platform", "win-64"])
        .env("CONDA_OVERRIDE_CUDA", "50")
        .env("CONDA_OVERRIDE_CLI_CAPABILITY", "8")
        .output()
        .unwrap();
    assert_consumer_json(&output);
    harness.assert_not_executed();
}

#[tokio::test]
async fn detector_failures_and_invalid_overrides_reach_stderr() {
    let harness = Harness::new(
        Some("allow"),
        Some("#!/bin/sh\nprintf 'executed\\n' >> \"$CLI_DETECTOR_MARKER\"\necho cli-detector-failure >&2\nexit 17\n"),
    )
    .await;
    for command in ["solve", "create"] {
        let output = harness
            .command(command)
            .arg("cli-consumer")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("cli-detector-failure"), "{stderr}");
        assert!(stderr.contains("cli-detect"), "{stderr}");
        assert!(!harness.prefix().exists());
    }
    let invalid = harness
        .command("solve")
        .env("CONDA_OVERRIDE_CLI_CAPABILITY", "7=bad build")
        .arg("cli-consumer")
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("CONDA_OVERRIDE_CLI_CAPABILITY"));
    assert!(invalid.stdout.is_empty());
}

#[tokio::test]
async fn demand_includes_constraints_but_skips_unrelated_detectors() {
    let unrelated = Harness::new(Some("allow"), None).await;
    let output = unrelated
        .command("solve")
        .arg("cli-unrelated")
        .output()
        .unwrap();
    assert_success(&output);
    let records: Vec<rattler_conda_types::RepoDataRecord> =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].package_record.name.as_normalized(),
        "cli-unrelated"
    );
    unrelated.assert_not_executed();

    for args in [
        vec!["cli-constrained"],
        vec!["cli-unrelated", "--constraint", "__cli_capability >=8"],
    ] {
        let harness = Harness::new(Some("allow"), None).await;
        let output = harness.command("solve").args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("__cli_capability"));
        assert_eq!(
            std::fs::read_to_string(harness.marker()).unwrap(),
            "executed\n"
        );
    }
}
