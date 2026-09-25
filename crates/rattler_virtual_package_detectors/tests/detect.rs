//! End-to-end tests against a channel of generated detector packages.
//!
//! Every test builds its own channel in a temporary directory: `noarch:
//! generic` packages whose executables are shell scripts on Unix and batch
//! files on Windows, indexed with `rattler_index` so the repodata carries real
//! hashes and the `info.virtual_package_detectors` registrations.

use std::{
    collections::{BTreeSet, HashMap},
    fs::File,
    path::{Path, PathBuf},
    time::Duration,
};

use rattler_cache::package_cache::PackageCache;
use rattler_conda_types::compression_level::CompressionLevel;
use rattler_conda_types::{
    Channel, GenericVirtualPackage, PackageName, Subdir,
    package::{IndexJson, PathType, PathsEntry, PathsJson},
};
use rattler_digest::{Sha256, compute_bytes_digest};
use rattler_index::{
    ChannelMetadata, IndexFsConfig, PackageRevisionAssignment, index_fs_with_channel_metadata,
};
use rattler_networking::LazyClient;
use rattler_package_streaming::write::write_tar_bz2_package;
use rattler_repodata_gateway::{AcceptedDetectorRegistration, Gateway};
use rattler_virtual_package_detectors::{
    AllowAll, CacheClock, DenyAll, DetectError, DetectOptions, DetectedValue, DetectionOutcome,
    DetectionSource, DetectorConsent, RunError, SkipReason, WantedNames, detect,
};

/// A detector package: an executable and the virtual packages it reports.
struct Fixture {
    name: &'static str,
    depends: &'static [&'static str],
    /// The body of the Unix script, after the shebang.
    sh: &'static str,
    /// The body of the Windows batch file, after `@echo off`.
    bat: &'static str,
    /// The registered names; `None` leaves the package unregistered.
    registers: Option<&'static [&'static str]>,
}

const GOOD_REPORT_SH: &str = r#"echo "{\"version\": 1, \"virtual_packages\": {\"__test_good\": {\"version\": \"1.2.3\"}, \"__test_absent\": null}, \"cache\": {\"ttl_seconds\": 3600, \"watch_env\": [\"DETECTOR_TEST_WATCH\"]}}""#;
const GOOD_REPORT_BAT: &str = r#"echo {"version": 1, "virtual_packages": {"__test_good": {"version": "1.2.3"}, "__test_absent": null}, "cache": {"ttl_seconds": 3600, "watch_env": ["DETECTOR_TEST_WATCH"]}}"#;

const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "good-detect",
        depends: &["detect-lib"],
        sh: GOOD_REPORT_SH,
        bat: GOOD_REPORT_BAT,
        registers: Some(&["__test_good", "__test_absent"]),
    },
    Fixture {
        // Reports whether the dependency's activation script ran.
        name: "activated-detect",
        depends: &["detect-lib"],
        sh: r#"echo "{\"version\": 1, \"virtual_packages\": {\"__test_activated\": {\"version\": \"${DETECT_LIB_ACTIVATED:-0}\"}}}""#,
        bat: r#"if "%DETECT_LIB_ACTIVATED%"=="" (set DETECT_LIB_ACTIVATED=0)
echo {"version": 1, "virtual_packages": {"__test_activated": {"version": "%DETECT_LIB_ACTIVATED%"}}}"#,
        registers: Some(&["__test_activated"]),
    },
    Fixture {
        name: "bad-exit-detect",
        depends: &[],
        sh: "echo boom >&2\nexit 2",
        bat: "echo boom 1>&2\r\nexit /b 2",
        registers: Some(&["__test_bad_exit"]),
    },
    Fixture {
        name: "malformed-detect",
        depends: &[],
        sh: "echo not json",
        bat: "echo not json",
        registers: Some(&["__test_malformed"]),
    },
    Fixture {
        name: "undeclared-detect",
        depends: &[],
        sh: r#"echo "{\"version\": 1, \"virtual_packages\": {\"__test_undeclared\": null, \"__test_extra\": null}}""#,
        bat: r#"echo {"version": 1, "virtual_packages": {"__test_undeclared": null, "__test_extra": null}}"#,
        registers: Some(&["__test_undeclared"]),
    },
    Fixture {
        name: "slow-detect",
        depends: &[],
        sh: "sleep 30",
        bat: "ping -n 31 127.0.0.1 >nul",
        registers: Some(&["__test_slow"]),
    },
    Fixture {
        // A dependency with an activation script and no executable.
        name: "detect-lib",
        depends: &[],
        sh: "",
        bat: "",
        registers: None,
    },
];

fn sha256_of(bytes: &[u8]) -> rattler_digest::Sha256Hash {
    compute_bytes_digest::<Sha256>(bytes)
}

/// Writes one fixture as a `noarch: generic` package into `subdir`.
fn write_package(fixture: &Fixture, subdir: &Path, staging: &Path) {
    let base = staging.join(fixture.name);
    let info = base.join("info");
    std::fs::create_dir_all(&info).unwrap();
    let mut files: Vec<(PathBuf, Vec<u8>)> = Vec::new();

    if fixture.name == "detect-lib" {
        files.push((
            PathBuf::from("etc/conda/activate.d/detect-lib.sh"),
            b"export DETECT_LIB_ACTIVATED=1\n".to_vec(),
        ));
        files.push((
            PathBuf::from("etc/conda/activate.d/detect-lib.bat"),
            b"@set DETECT_LIB_ACTIVATED=1\r\n".to_vec(),
        ));
    } else {
        files.push((
            PathBuf::from("bin").join(fixture.name),
            format!("#!/bin/sh\n{}\n", fixture.sh).into_bytes(),
        ));
        files.push((
            PathBuf::from("Scripts").join(format!("{}.bat", fixture.name)),
            format!("@echo off\r\n{}\r\n", fixture.bat).into_bytes(),
        ));
    }

    let mut paths = Vec::new();
    for (relative, contents) in &files {
        let path = base.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        paths.push(PathsEntry {
            relative_path: relative.clone(),
            no_link: false,
            path_type: PathType::HardLink,
            prefix_placeholder: None,
            sha256: Some(sha256_of(contents)),
            size_in_bytes: Some(contents.len() as u64),
        });
    }

    let index = IndexJson {
        arch: None,
        build: "0".to_string(),
        build_number: 0,
        constrains: Vec::new(),
        depends: fixture.depends.iter().map(ToString::to_string).collect(),
        extra_depends: std::collections::BTreeMap::default(),
        features: None,
        flags: Vec::new(),
        license: None,
        license_family: None,
        name: PackageName::try_from(fixture.name).unwrap(),
        noarch: rattler_conda_types::NoArchType::generic(),
        platform: None,
        purls: None,
        python_site_packages_path: None,
        repodata_revision: None,
        subdir: Some("noarch".to_string()),
        timestamp: Some(jiff::Timestamp::from_second(1_700_000_000).unwrap().into()),
        track_features: Vec::new(),
        version: "1.0.0".parse().unwrap(),
    };
    std::fs::write(
        info.join("index.json"),
        serde_json::to_vec_pretty(&index).unwrap(),
    )
    .unwrap();
    std::fs::write(
        info.join("paths.json"),
        serde_json::to_vec_pretty(&PathsJson {
            paths,
            paths_version: 1,
        })
        .unwrap(),
    )
    .unwrap();

    let mut archive_paths: Vec<PathBuf> = files
        .iter()
        .map(|(relative, _)| base.join(relative))
        .collect();
    archive_paths.push(info.join("index.json"));
    archive_paths.push(info.join("paths.json"));
    let writer = File::create(subdir.join(format!("{}-1.0.0-0.tar.bz2", fixture.name))).unwrap();
    write_tar_bz2_package(
        writer,
        &base,
        &archive_paths,
        CompressionLevel::Default,
        None,
        None,
    )
    .unwrap();
}

/// A channel with every fixture, indexed and registered.
async fn build_channel(root: &Path) -> Channel {
    let channel = root.join("channel");
    let noarch = channel.join("noarch");
    std::fs::create_dir_all(&noarch).unwrap();
    let staging = root.join("staging");
    for fixture in FIXTURES {
        write_package(fixture, &noarch, &staging);
    }
    let registrations: serde_json::Map<String, serde_json::Value> = FIXTURES
        .iter()
        .filter_map(|fixture| {
            fixture
                .registers
                .map(|names| (fixture.name.to_string(), serde_json::json!(names)))
        })
        .collect();
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
            virtual_package_detectors: Some(serde_json::Value::Object(registrations)),
            ..ChannelMetadata::default()
        },
    )
    .await
    .unwrap();
    Channel::try_from_directory(&channel).unwrap()
}

struct Harness {
    _dir: tempfile::TempDir,
    root: PathBuf,
    gateway: Gateway,
    package_cache: PackageCache,
    channel: Channel,
    host: Subdir,
}

impl Harness {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let channel = build_channel(dir.path()).await;
        let package_cache = PackageCache::new(dir.path().join("pkgs"));
        let gateway = Gateway::builder()
            .with_cache_dir(dir.path().join("repodata"))
            .with_package_cache(package_cache.clone())
            .finish();
        Self {
            root: dir.path().join("detectors"),
            _dir: dir,
            gateway,
            package_cache,
            channel,
            host: Subdir::current().unwrap(),
        }
    }

    async fn registrations(&self) -> Vec<AcceptedDetectorRegistration> {
        let output = self
            .gateway
            .virtual_package_detectors([self.channel.clone()], self.host)
            .await
            .unwrap();
        assert!(output.warnings.is_empty(), "{:?}", output.warnings);
        output.registrations
    }

    fn only(
        registrations: Vec<AcceptedDetectorRegistration>,
        names: &[&str],
    ) -> Vec<AcceptedDetectorRegistration> {
        registrations
            .into_iter()
            .filter(|r| names.contains(&r.registration.detector.as_source()))
            .collect()
    }

    fn options<'a>(&'a self, consent: &'a dyn DetectorConsent) -> DetectOptions<'a> {
        DetectOptions {
            gateway: &self.gateway,
            package_cache: &self.package_cache,
            download_client: LazyClient::default(),
            root: &self.root,
            host_platform: self.host,
            target_platform: self.host,
            client_virtual_packages: Vec::new(),
            timeout: Duration::from_secs(60),
            consent,
            wanted: WantedNames::All,
            concurrency: 4,
            clock: CacheClock::current(),
        }
    }

    async fn run(&self, detectors: &[&str], consent: &dyn DetectorConsent) -> DetectionOutcome {
        let registrations = Self::only(self.registrations().await, detectors);
        detect(&registrations, self.options(consent)).await.unwrap()
    }
}

fn values(outcome: &DetectionOutcome) -> HashMap<String, Option<String>> {
    outcome
        .results
        .iter()
        .map(|result| {
            (
                result.name.as_normalized().to_string(),
                match &result.value {
                    DetectedValue::Absent => None,
                    DetectedValue::Present(version) => Some(version.version.to_string()),
                },
            )
        })
        .collect()
}

fn failed(outcome: &DetectionOutcome) -> Vec<(&str, &DetectError, Option<&str>)> {
    outcome
        .failures
        .iter()
        .map(|failure| {
            (
                failure.detector.as_source(),
                &failure.error,
                failure.stderr.as_deref(),
            )
        })
        .collect()
}

#[tokio::test]
async fn detects_caches_and_activates() {
    let harness = Harness::new().await;
    let outcome = harness
        .run(&["good-detect", "activated-detect"], &AllowAll)
        .await;
    assert!(outcome.failures.is_empty(), "{:?}", failed(&outcome));
    assert!(outcome.skipped.is_empty());
    assert_eq!(
        values(&outcome),
        HashMap::from([
            ("__test_good".to_string(), Some("1.2.3".to_string())),
            ("__test_absent".to_string(), None),
            ("__test_activated".to_string(), Some("1".to_string())),
        ])
    );
    let good = outcome
        .results
        .iter()
        .find(|r| r.name.as_normalized() == "__test_good")
        .unwrap();
    match &good.source {
        DetectionSource::Detector {
            detector,
            from_cache,
            origin,
            ..
        } => {
            assert_eq!(detector.as_source(), "good-detect");
            assert!(!from_cache);
            assert_eq!(origin, &harness.channel.base_url);
        }
        other @ DetectionSource::Override { .. } => panic!("unexpected source {other:?}"),
    }
    // Both detectors share the dependency but have different digests, so two
    // environments exist.
    assert_eq!(
        std::fs::read_dir(harness.root.join("envs"))
            .unwrap()
            .count(),
        2
    );

    // A second run is served from the cache without reinstalling.
    let again = harness
        .run(&["good-detect", "activated-detect"], &AllowAll)
        .await;
    assert_eq!(values(&again), values(&outcome));
    assert!(again.results.iter().all(|r| matches!(
        r.source,
        DetectionSource::Detector {
            from_cache: true,
            ..
        }
    )));
    assert_eq!(
        std::fs::read_dir(harness.root.join("envs"))
            .unwrap()
            .count(),
        2
    );
}

#[tokio::test]
async fn failures_discard_the_detector_and_keep_the_others() {
    let harness = Harness::new().await;
    let outcome = harness
        .run(
            &[
                "good-detect",
                "bad-exit-detect",
                "malformed-detect",
                "undeclared-detect",
            ],
            &AllowAll,
        )
        .await;
    assert_eq!(
        values(&outcome),
        HashMap::from([
            ("__test_good".to_string(), Some("1.2.3".to_string())),
            ("__test_absent".to_string(), None),
        ])
    );
    assert_eq!(
        outcome
            .failures
            .iter()
            .map(|failure| failure.detector.as_normalized())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["bad-exit-detect", "malformed-detect", "undeclared-detect"])
    );
}

#[tokio::test]
async fn denied_consent_skips_without_installing() {
    let harness = Harness::new().await;
    let outcome = harness.run(&["good-detect"], &DenyAll).await;
    assert!(outcome.results.is_empty());
    assert!(outcome.failures.is_empty());
    assert_eq!(outcome.skipped.len(), 1);
    assert_eq!(outcome.skipped[0].reason, SkipReason::ConsentDenied);
    assert!(!harness.root.join("envs").exists());
}

#[tokio::test]
async fn overrides_replace_results_and_can_make_a_detector_unnecessary() {
    let harness = Harness::new().await;
    // SAFETY: nextest runs every test in its own process.
    unsafe { std::env::set_var("CONDA_OVERRIDE_TEST_GOOD", "9.9=custom") };
    let outcome = harness.run(&["good-detect"], &AllowAll).await;
    assert_eq!(
        values(&outcome),
        HashMap::from([
            ("__test_good".to_string(), Some("9.9".to_string())),
            ("__test_absent".to_string(), None),
        ])
    );
    let good = outcome
        .results
        .iter()
        .find(|r| r.name.as_normalized() == "__test_good")
        .unwrap();
    assert_eq!(
        good.source,
        DetectionSource::Override {
            variable: "CONDA_OVERRIDE_TEST_GOOD".to_string()
        }
    );
    match &good.value {
        DetectedValue::Present(version) => assert_eq!(version.build_string, "custom"),
        DetectedValue::Absent => panic!("override should be present"),
    }

    // Overriding every name leaves nothing for the detector to do.
    unsafe { std::env::set_var("CONDA_OVERRIDE_TEST_ABSENT", "") };
    let outcome = harness.run(&["good-detect"], &AllowAll).await;
    assert_eq!(outcome.skipped.len(), 1);
    assert_eq!(outcome.skipped[0].reason, SkipReason::NoWantedName);
    assert_eq!(
        values(&outcome),
        HashMap::from([
            ("__test_good".to_string(), Some("9.9".to_string())),
            ("__test_absent".to_string(), None),
        ])
    );

    // An invalid override is an error.
    unsafe { std::env::set_var("CONDA_OVERRIDE_TEST_ABSENT", "not a version!") };
    let registrations = Harness::only(harness.registrations().await, &["good-detect"]);
    let err = detect(&registrations, harness.options(&AllowAll))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("CONDA_OVERRIDE_TEST_ABSENT"),
        "{err}"
    );
}

#[tokio::test]
async fn foreign_target_and_unwanted_names_skip() {
    let harness = Harness::new().await;
    let registrations = Harness::only(harness.registrations().await, &["good-detect"]);

    let mut options = harness.options(&AllowAll);
    options.target_platform = if harness.host == Subdir::Linux64 {
        Subdir::LinuxAarch64
    } else {
        Subdir::Linux64
    };
    let outcome = detect(&registrations, options).await.unwrap();
    assert_eq!(
        outcome.skipped[0].reason,
        SkipReason::TargetIsNotHost {
            override_variables: vec![
                "CONDA_OVERRIDE_TEST_GOOD".to_string(),
                "CONDA_OVERRIDE_TEST_ABSENT".to_string()
            ]
        }
    );
    assert!(outcome.results.is_empty());

    let mut options = harness.options(&AllowAll);
    options.wanted = WantedNames::Only(BTreeSet::from([
        PackageName::try_from("__unrelated").unwrap()
    ]));
    let outcome = detect(&registrations, options).await.unwrap();
    assert_eq!(outcome.skipped[0].reason, SkipReason::NoWantedName);

    let mut options = harness.options(&AllowAll);
    options.wanted = WantedNames::Only(BTreeSet::from([
        PackageName::try_from("__test_absent").unwrap()
    ]));
    let outcome = detect(&registrations, options).await.unwrap();
    assert!(outcome.skipped.is_empty());
    assert_eq!(outcome.results.len(), 2);
}

#[tokio::test]
async fn timeout_terminates_a_slow_detector() {
    let harness = Harness::new().await;
    let registrations = Harness::only(harness.registrations().await, &["slow-detect"]);
    let mut options = harness.options(&AllowAll);
    options.timeout = Duration::from_secs(1);
    let started = std::time::Instant::now();
    let outcome = detect(&registrations, options).await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(outcome.failures.len(), 1);
    assert!(matches!(
        outcome.failures[0].error,
        DetectError::Run(RunError::TimedOut { .. })
    ));
}

#[tokio::test]
async fn results_merge_over_client_virtual_packages() {
    let harness = Harness::new().await;
    let outcome = harness.run(&["good-detect"], &AllowAll).await;
    let client = vec![GenericVirtualPackage {
        name: PackageName::try_from("__test_good").unwrap(),
        version: "0.1".parse().unwrap(),
        build_string: "0".to_string(),
    }];
    let merged = rattler_virtual_package_detectors::merge_results(client, &outcome.results);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].version.to_string(), "1.2.3");
}
