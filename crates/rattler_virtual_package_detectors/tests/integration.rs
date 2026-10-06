//! Integration tests covering `detect` and the gateway's
//! registration query.
//!
//! Every test builds its own channels in a temporary directory from generated
//! `noarch: generic` packages whose executables are shell scripts, which is why
//! the whole file is Unix only. Channels are indexed with `rattler_index` so
//! the repodata carries real hashes, `info.virtual_package_detectors` and,
//! where a test needs them, `info.channel_relations` or sharded repodata.

#![cfg(unix)]

use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use rattler_boot_id::BootId;
use rattler_cache::package_cache::PackageCache;
use rattler_conda_types::{
    Channel, ChannelRelations, PackageName, Subdir,
    compression_level::CompressionLevel,
    package::{IndexJson, PathType, PathsEntry, PathsJson},
};
use rattler_digest::{Sha256, compute_bytes_digest};
use rattler_index::{
    ChannelMetadata, IndexFsConfig, PackageRevisionAssignment, index_fs_with_channel_metadata,
};
use rattler_networking::LazyClient;
use rattler_package_streaming::write::write_tar_bz2_package;
use rattler_repodata_gateway::{
    AcceptedDetectorRegistration, Gateway, RegistrationConflictKind, VirtualPackageDetectorsOutput,
};
use rattler_virtual_package_detectors::EnvironmentSnapshot;
use rattler_virtual_package_detectors::{
    ActivationError, AllowAll, CacheClock, DetectError, DetectOptions, DetectedValue,
    DetectionOutcome, DetectionSource, DetectorConsent, DetectorEnvironment,
    DetectorEnvironmentProvider, EnvironmentError, EnvironmentOptions, RattlerEnvironmentProvider,
    ResolvedDetector, RunError, SkipReason, WantedNames, detect,
    limits::{MAX_CACHE_LIFETIME, OUTPUT_LIMIT, REBOOT_FALLBACK_LIFETIME},
};
use url::Url;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A generated `noarch: generic` package.
#[derive(Clone, Debug)]
struct Package {
    name: String,
    version: String,
    build: String,
    depends: Vec<String>,
    /// Files relative to the prefix, with their contents and mode.
    files: Vec<(PathBuf, Vec<u8>, u32)>,
}

impl Package {
    /// A detector whose `bin/<name>` is a `/bin/sh` script with `body`.
    fn detector(name: &str, body: &str) -> Self {
        Self {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            build: "0".to_string(),
            depends: Vec::new(),
            files: vec![(
                PathBuf::from("bin").join(name),
                format!("#!/bin/sh\n{body}\n").into_bytes(),
                0o755,
            )],
        }
    }

    /// A dependency without an executable.
    fn library(name: &str) -> Self {
        Self {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            build: "0".to_string(),
            depends: Vec::new(),
            files: Vec::new(),
        }
    }

    fn version(mut self, version: &str) -> Self {
        self.version = version.to_string();
        self
    }

    fn depends(mut self, depends: &[&str]) -> Self {
        self.depends = depends.iter().map(ToString::to_string).collect();
        self
    }

    fn file(mut self, path: &str, contents: &str) -> Self {
        self.files
            .push((PathBuf::from(path), contents.as_bytes().to_vec(), 0o755));
        self
    }

    fn file_with_mode(mut self, path: &str, contents: &str, mode: u32) -> Self {
        self.files
            .push((PathBuf::from(path), contents.as_bytes().to_vec(), mode));
        self
    }

    fn file_name(&self) -> String {
        format!("{}-{}-{}.tar.bz2", self.name, self.version, self.build)
    }
}

/// A shell snippet printing a report with exactly the given results, each
/// `null` or `{"version": ...}`; values are inserted verbatim so they may use
/// shell expansions.
fn report(results: &[(&str, &str)]) -> String {
    let entries: Vec<String> = results
        .iter()
        .map(|(name, value)| format!("\\\"{name}\\\": {value}"))
        .collect();
    format!(
        "echo \"{{\\\"version\\\": 1, \\\"virtual_packages\\\": {{{}}}}}\"",
        entries.join(", ")
    )
}

/// A report with a `cache` object, `cache` being the JSON body of that object
/// with double quotes escaped for the enclosing `echo "..."`.
fn report_with_cache(results: &[(&str, &str)], cache: &str) -> String {
    let entries: Vec<String> = results
        .iter()
        .map(|(name, value)| format!("\\\"{name}\\\": {value}"))
        .collect();
    format!(
        "echo \"{{\\\"version\\\": 1, \\\"virtual_packages\\\": {{{}}}, \\\"cache\\\": {{{cache}}}}}\"",
        entries.join(", ")
    )
}

fn present(version: &str) -> String {
    format!("{{\\\"version\\\": \\\"{version}\\\"}}")
}

fn write_package(package: &Package, subdir: &Path, staging: &Path) {
    let base = staging.join(format!(
        "{}-{}-{}",
        package.name, package.version, package.build
    ));
    if base.exists() {
        std::fs::remove_dir_all(&base).unwrap();
    }
    let info = base.join("info");
    std::fs::create_dir_all(&info).unwrap();

    let mut paths = Vec::new();
    for (relative, contents, mode) in &package.files {
        let path = base.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(*mode)).unwrap();
        paths.push(PathsEntry {
            relative_path: relative.clone(),
            no_link: false,
            path_type: PathType::HardLink,
            prefix_placeholder: None,
            sha256: Some(compute_bytes_digest::<Sha256>(contents)),
            size_in_bytes: Some(contents.len() as u64),
        });
    }

    let index = IndexJson {
        arch: None,
        build: package.build.clone(),
        build_number: 0,
        constrains: Vec::new(),
        depends: package.depends.clone(),
        extra_depends: BTreeMap::default(),
        features: None,
        flags: Vec::new(),
        license: None,
        license_family: None,
        name: PackageName::try_from(package.name.as_str()).unwrap(),
        noarch: rattler_conda_types::NoArchType::generic(),
        platform: None,
        purls: None,
        python_site_packages_path: None,
        repodata_revision: None,
        subdir: Some("noarch".to_string()),
        timestamp: Some(jiff::Timestamp::from_second(1_700_000_000).unwrap().into()),
        track_features: Vec::new(),
        version: package.version.parse().unwrap(),
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

    let mut archive_paths: Vec<PathBuf> = package
        .files
        .iter()
        .map(|(relative, _, _)| base.join(relative))
        .collect();
    archive_paths.push(info.join("index.json"));
    archive_paths.push(info.join("paths.json"));
    std::fs::create_dir_all(subdir).unwrap();
    let writer = File::create(subdir.join(package.file_name())).unwrap();
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

/// What to publish in a channel.
#[derive(Default)]
struct ChannelSpec {
    packages: Vec<Package>,
    /// The `info.virtual_package_detectors` of `noarch`.
    noarch_registrations: Option<serde_json::Value>,
    /// The `info.virtual_package_detectors` of the host platform's subdir;
    /// that subdir only exists when this is set.
    platform_registrations: Option<serde_json::Value>,
    relations: Option<ChannelRelations>,
    write_shards: bool,
}

fn registrations(entries: &[(&str, &[&str])]) -> serde_json::Value {
    serde_json::Value::Object(
        entries
            .iter()
            .map(|(detector, names)| (detector.to_string(), serde_json::json!(names)))
            .collect(),
    )
}

async fn index_subdir(
    channel: &Path,
    platform: Subdir,
    detectors: Option<serde_json::Value>,
    relations: Option<ChannelRelations>,
    write_shards: bool,
) {
    std::fs::create_dir_all(channel.join(platform.as_str())).unwrap();
    index_fs_with_channel_metadata(
        IndexFsConfig {
            channel: channel.to_path_buf(),
            target_platform: Some(platform),
            repodata_patch: None,
            write_zst: false,
            write_shards,
            repodata_revisions: Vec::new(),
            package_revision_assignment: PackageRevisionAssignment::FromIndexJson,
            force: true,
            max_parallel: 1,
            multi_progress: None,
        },
        ChannelMetadata {
            virtual_package_detectors: detectors
                .map(|value| serde_json::from_value(value).unwrap()),
            channel_relations: relations,
            ..ChannelMetadata::default()
        },
    )
    .await
    .unwrap();
}

/// Writes and indexes a channel at `dir`. Calling it again re-indexes.
async fn build_channel(dir: &Path, spec: &ChannelSpec) -> Channel {
    let noarch = dir.join("noarch");
    let staging = dir.parent().unwrap().join(format!(
        "{}-staging",
        dir.file_name().unwrap().to_string_lossy()
    ));
    for package in &spec.packages {
        write_package(package, &noarch, &staging);
    }
    index_subdir(
        dir,
        Subdir::NoArch,
        spec.noarch_registrations.clone(),
        spec.relations.clone(),
        spec.write_shards,
    )
    .await;
    if let Some(platform_registrations) = &spec.platform_registrations {
        index_subdir(
            dir,
            Subdir::current().unwrap(),
            Some(platform_registrations.clone()),
            spec.relations.clone(),
            spec.write_shards,
        )
        .await;
    }
    Channel::try_from_directory(dir).unwrap()
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Harness {
    _dir: tempfile::TempDir,
    dir: PathBuf,
    root: PathBuf,
    gateway: Gateway,
    package_cache: PackageCache,
    host: Subdir,
    environment: EnvironmentSnapshot,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let package_cache = PackageCache::new(dir.path().join("pkgs"));
        let gateway = Self::gateway(dir.path(), &package_cache);
        Self {
            dir: dir.path().to_path_buf(),
            root: dir.path().join("detectors"),
            _dir: dir,
            gateway,
            package_cache,
            host: Subdir::current().unwrap(),
            environment: EnvironmentSnapshot::from_system(),
        }
    }

    fn gateway(dir: &Path, package_cache: &PackageCache) -> Gateway {
        Gateway::builder()
            .with_cache_dir(dir.join("repodata"))
            .with_package_cache(package_cache.clone())
            .finish()
    }

    /// Replaces the gateway so re-indexed repodata is read again; the
    /// package cache, environments and result cache stay.
    fn fresh_gateway(&mut self) {
        self.gateway = Self::gateway(&self.dir, &self.package_cache);
    }

    async fn channel(&self, name: &str, spec: &ChannelSpec) -> Channel {
        build_channel(&self.dir.join(name), spec).await
    }

    async fn query(&self, channels: &[Channel]) -> VirtualPackageDetectorsOutput {
        self.gateway
            .virtual_package_detectors(channels.iter().cloned(), [self.host, Subdir::NoArch])
            .await
            .unwrap()
    }

    async fn registrations(&self, channels: &[Channel]) -> Vec<AcceptedDetectorRegistration> {
        self.query(channels).await.registrations
    }

    fn options<'a>(&'a self, consent: &'a dyn DetectorConsent) -> DetectOptions<'a> {
        DetectOptions {
            environment_provider: self,
            environment: &self.environment,
            root: &self.root,
            host_platform: self.host,
            target_platform: self.host,
            timeout: Duration::from_secs(60),
            consent,
            wanted: WantedNames::All,
            concurrency: 4,
            clock: CacheClock::current(),
        }
    }

    async fn detect(&self, registrations: &[AcceptedDetectorRegistration]) -> DetectionOutcome {
        detect(registrations, self.options(&AllowAll))
            .await
            .unwrap()
    }

    fn envs(&self) -> Vec<PathBuf> {
        match std::fs::read_dir(self.root.join("envs")) {
            Ok(entries) => entries.map(|entry| entry.unwrap().path()).collect(),
            Err(_) => Vec::new(),
        }
    }

    fn cached_results(&self) -> Vec<serde_json::Value> {
        match std::fs::read_dir(self.root.join("results")) {
            Ok(entries) => entries
                .map(|entry| {
                    serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

#[async_trait]
impl DetectorEnvironmentProvider for Harness {
    async fn resolve(
        &self,
        registration: &AcceptedDetectorRegistration,
    ) -> Result<ResolvedDetector, EnvironmentError> {
        let root = self.root.join("envs");
        RattlerEnvironmentProvider::new(EnvironmentOptions {
            gateway: &self.gateway,
            package_cache: &self.package_cache,
            download_client: LazyClient::default(),
            root: &root,
            host_platform: self.host,
            virtual_packages: Vec::new(),
        })
        .resolve(registration)
        .await
    }

    async fn install(
        &self,
        resolved: ResolvedDetector,
    ) -> Result<DetectorEnvironment, EnvironmentError> {
        let root = self.root.join("envs");
        RattlerEnvironmentProvider::new(EnvironmentOptions {
            gateway: &self.gateway,
            package_cache: &self.package_cache,
            download_client: LazyClient::default(),
            root: &root,
            host_platform: self.host,
            virtual_packages: Vec::new(),
        })
        .install(resolved)
        .await
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

fn failures(outcome: &DetectionOutcome) -> HashMap<String, &DetectError> {
    outcome
        .failures
        .iter()
        .map(|failure| (failure.detector.as_source().to_string(), &failure.error))
        .collect()
}

fn from_cache(outcome: &DetectionOutcome, name: &str) -> bool {
    match &outcome
        .results
        .iter()
        .find(|result| result.name.as_normalized() == name)
        .unwrap_or_else(|| panic!("no result for {name} in {:?}", values(outcome)))
        .source
    {
        DetectionSource::Detector { from_cache, .. } => *from_cache,
        DetectionSource::Override { .. } => panic!("{name} came from an override"),
    }
}

fn digest_of(outcome: &DetectionOutcome, name: &str) -> String {
    match &outcome
        .results
        .iter()
        .find(|result| result.name.as_normalized() == name)
        .unwrap()
        .source
    {
        DetectionSource::Detector { digest, .. } => hex::encode(digest),
        DetectionSource::Override { .. } => panic!("{name} came from an override"),
    }
}

fn count_lines(path: &Path) -> usize {
    std::fs::read_to_string(path).map_or(0, |contents| contents.lines().count())
}

// ---------------------------------------------------------------------------
// Concurrency
// ---------------------------------------------------------------------------

/// Several detections of the same registration against the same root at the
/// same time must install the environment exactly once and all succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_detections_install_the_environment_once() {
    const TASKS: usize = 8;
    let mut harness = Harness::new();
    let log = harness.dir.join("invocations.log");
    harness
        .environment
        .insert("DETECTOR_TEST_LOG", log.to_str().unwrap());
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![
                    Package::detector(
                        "conc-detect",
                        &format!(
                            "echo run >> \"$DETECTOR_TEST_LOG\"\n{}",
                            report(&[("__test_conc", &present("${DETECT_LIB_ACTIVATED:-0}"))])
                        ),
                    )
                    .depends(&["detect-lib"]),
                    Package::library("detect-lib").file(
                        "etc/conda/activate.d/detect-lib.sh",
                        "export DETECT_LIB_ACTIVATED=1\n",
                    ),
                ],
                noarch_registrations: Some(registrations(&[("conc-detect", &["__test_conc"])])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let registrations = Arc::new(harness.registrations(&[channel]).await);
    assert_eq!(registrations.len(), 1);
    let harness = Arc::new(harness);
    let mut handles = Vec::new();
    for _ in 0..TASKS {
        let harness = harness.clone();
        let registrations = registrations.clone();
        handles.push(tokio::spawn(
            async move { harness.detect(&registrations).await },
        ));
    }
    let mut outcomes = Vec::new();
    for handle in handles {
        outcomes.push(handle.await.unwrap());
    }

    for outcome in &outcomes {
        assert!(
            outcome.failures.is_empty(),
            "a concurrent run failed: {:?}",
            outcome.failures
        );
        assert!(outcome.skipped.is_empty());
        assert_eq!(
            values(outcome),
            HashMap::from([("__test_conc".to_string(), Some("1".to_string()))])
        );
    }
    let envs = harness.envs();
    assert_eq!(envs.len(), 1, "expected one environment, found {envs:?}");
    assert!(envs[0].join("bin/conc-detect").is_file());
    let runs = count_lines(&log);
    assert!(
        (1..=TASKS).contains(&runs),
        "the detector ran {runs} times for {TASKS} concurrent detections"
    );
    // Once cached, nothing runs again.
    let again = harness.detect(&registrations).await;
    assert!(from_cache(&again, "__test_conc"));
    assert_eq!(count_lines(&log), runs);
}

// ---------------------------------------------------------------------------
// Digest changes
// ---------------------------------------------------------------------------

/// A new version of the detector gets a new environment and must not be
/// served the cached result of the old digest.
#[tokio::test]
async fn a_new_detector_version_gets_a_new_environment_and_a_cache_miss() {
    let mut harness = Harness::new();
    let spec = ChannelSpec {
        packages: vec![Package::detector(
            "vers-detect",
            &report(&[("__test_vers", &present("1.0.0"))]),
        )],
        noarch_registrations: Some(registrations(&[("vers-detect", &["__test_vers"])])),
        ..ChannelSpec::default()
    };
    let channel = harness.channel("channel", &spec).await;
    let registrations = harness.registrations(std::slice::from_ref(&channel)).await;
    let first = harness.detect(&registrations).await;
    assert_eq!(values(&first)["__test_vers"], Some("1.0.0".to_string()));
    assert!(!from_cache(&first, "__test_vers"));
    let cached = harness.detect(&registrations).await;
    assert!(from_cache(&cached, "__test_vers"));

    // Publish 2.0.0 next to 1.0.0 and re-index.
    let mut spec = spec;
    spec.packages.push(
        Package::detector(
            "vers-detect",
            &report(&[("__test_vers", &present("2.0.0"))]),
        )
        .version("2.0.0"),
    );
    harness.channel("channel", &spec).await;
    harness.fresh_gateway();
    let registrations = harness.registrations(std::slice::from_ref(&channel)).await;
    let second = harness.detect(&registrations).await;
    assert!(second.failures.is_empty(), "{:?}", second.failures);
    assert_eq!(values(&second)["__test_vers"], Some("2.0.0".to_string()));
    assert!(
        !from_cache(&second, "__test_vers"),
        "the old digest's cached result was reused for the new version"
    );
    assert_ne!(
        digest_of(&first, "__test_vers"),
        digest_of(&second, "__test_vers")
    );
    assert_eq!(harness.envs().len(), 2);
    assert_eq!(harness.cached_results().len(), 2);
    let third = harness.detect(&registrations).await;
    assert!(from_cache(&third, "__test_vers"));
    assert_eq!(values(&third)["__test_vers"], Some("2.0.0".to_string()));
}

/// Rebuilding the same file name with different contents changes the SHA-256
/// and therefore the digest; the stale package in the package cache must not
/// be installed into the new environment.
#[tokio::test]
async fn a_rebuilt_package_with_the_same_file_name_is_reinstalled() {
    let mut harness = Harness::new();
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![Package::detector(
                    "rebuild-detect",
                    &report(&[("__test_rebuild", &present("1"))]),
                )],
                noarch_registrations: Some(registrations(&[(
                    "rebuild-detect",
                    &["__test_rebuild"],
                )])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let accepted = harness.registrations(std::slice::from_ref(&channel)).await;
    let first = harness.detect(&accepted).await;
    assert_eq!(values(&first)["__test_rebuild"], Some("1".to_string()));

    harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![Package::detector(
                    "rebuild-detect",
                    &report(&[("__test_rebuild", &present("2"))]),
                )],
                noarch_registrations: Some(registrations(&[(
                    "rebuild-detect",
                    &["__test_rebuild"],
                )])),
                ..ChannelSpec::default()
            },
        )
        .await;
    harness.fresh_gateway();
    let accepted = harness.registrations(&[channel]).await;
    let second = harness.detect(&accepted).await;
    assert!(second.failures.is_empty(), "{:?}", second.failures);
    assert_ne!(
        digest_of(&first, "__test_rebuild"),
        digest_of(&second, "__test_rebuild"),
        "the digest did not change although the package hash did"
    );
    assert!(!from_cache(&second, "__test_rebuild"));
    assert_eq!(
        values(&second)["__test_rebuild"],
        Some("2".to_string()),
        "the stale package from the package cache was installed"
    );
    assert_eq!(harness.envs().len(), 2);
}

/// The cache key includes the registered names: re-registering the same
/// detector with more names must not serve the old, narrower result.
#[tokio::test]
async fn changing_the_registered_names_misses_the_cache() {
    let mut harness = Harness::new();
    harness.environment.insert(
        "DETECTOR_TEST_REPORT",
        r#"{"version": 1, "virtual_packages": {"__test_n1": null}}"#,
    );
    let package = Package::detector("names-detect", "echo \"$DETECTOR_TEST_REPORT\"");
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![package.clone()],
                noarch_registrations: Some(registrations(&[("names-detect", &["__test_n1"])])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let registrations1 = harness.registrations(std::slice::from_ref(&channel)).await;
    let first = harness.detect(&registrations1).await;
    assert_eq!(values(&first).len(), 1);
    assert!(from_cache(
        &harness.detect(&registrations1).await,
        "__test_n1"
    ));

    harness.environment.insert(
        "DETECTOR_TEST_REPORT",
        r#"{"version": 1, "virtual_packages": {"__test_n1": null, "__test_n2": {"version": "2"}}}"#,
    );
    harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![package],
                noarch_registrations: Some(registrations(&[(
                    "names-detect",
                    &["__test_n1", "__test_n2"],
                )])),
                ..ChannelSpec::default()
            },
        )
        .await;
    harness.fresh_gateway();
    let registrations2 = harness.registrations(&[channel]).await;
    let second = harness.detect(&registrations2).await;
    assert!(second.failures.is_empty(), "{:?}", second.failures);
    assert!(!from_cache(&second, "__test_n1"));
    assert_eq!(
        values(&second),
        HashMap::from([
            ("__test_n1".to_string(), None),
            ("__test_n2".to_string(), Some("2".to_string())),
        ])
    );
    // The same package, so the same environment.
    assert_eq!(harness.envs().len(), 1);
}

// ---------------------------------------------------------------------------
// Cache expiry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ttl_zero_reruns_the_detector_every_time_and_stores_nothing() {
    let mut harness = Harness::new();
    let log = harness.dir.join("invocations.log");
    harness
        .environment
        .insert("DETECTOR_TEST_LOG", log.to_str().unwrap());
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![Package::detector(
                    "ttl0-detect",
                    &format!(
                        "echo run >> \"$DETECTOR_TEST_LOG\"\n{}",
                        report_with_cache(
                            &[("__test_ttl0", &present("1"))],
                            "\\\"ttl_seconds\\\": 0"
                        )
                    ),
                )],
                noarch_registrations: Some(registrations(&[("ttl0-detect", &["__test_ttl0"])])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let registrations = harness.registrations(&[channel]).await;
    for run in 1..=3 {
        let outcome = harness.detect(&registrations).await;
        assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
        assert!(!from_cache(&outcome, "__test_ttl0"), "run {run} was cached");
        assert_eq!(count_lines(&log), run);
    }
    assert!(harness.cached_results().is_empty());
}

#[tokio::test]
async fn a_watched_variable_change_expires_the_cache() {
    let mut harness = Harness::new();
    harness.environment.insert("DETECTOR_TEST_WATCH", "one");
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![Package::detector(
                    "watch-detect",
                    &report_with_cache(
                        &[("__test_watch", &present("${DETECTOR_TEST_WATCH:-unset}"))],
                        "\\\"watch_env\\\": [\\\"DETECTOR_TEST_WATCH\\\"]",
                    ),
                )],
                noarch_registrations: Some(registrations(&[("watch-detect", &["__test_watch"])])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let registrations = harness.registrations(&[channel]).await;
    let first = harness.detect(&registrations).await;
    assert!(!from_cache(&first, "__test_watch"));
    assert_eq!(values(&first)["__test_watch"], Some("one".to_string()));
    assert!(from_cache(
        &harness.detect(&registrations).await,
        "__test_watch"
    ));

    harness.environment.insert("DETECTOR_TEST_WATCH", "two");
    let changed = harness.detect(&registrations).await;
    assert!(!from_cache(&changed, "__test_watch"));
    assert_eq!(values(&changed)["__test_watch"], Some("two".to_string()));
    assert!(from_cache(
        &harness.detect(&registrations).await,
        "__test_watch"
    ));

    harness.environment.remove("DETECTOR_TEST_WATCH");
    let unset = harness.detect(&registrations).await;
    assert!(!from_cache(&unset, "__test_watch"));
    assert_eq!(values(&unset)["__test_watch"], Some("unset".to_string()));
    assert!(from_cache(
        &harness.detect(&registrations).await,
        "__test_watch"
    ));
}

/// Detects and reports whether the watched-path result came from the cache.
async fn cached(harness: &Harness, registrations: &[AcceptedDetectorRegistration]) -> bool {
    let outcome = harness.detect(registrations).await;
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    from_cache(&outcome, "__test_watchpath")
}

#[tokio::test]
async fn a_watched_path_change_expires_the_cache() {
    let mut harness = Harness::new();
    let watched = harness.dir.join("watched-file");
    harness
        .environment
        .insert("DETECTOR_TEST_WATCH_PATH", watched.to_str().unwrap());
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![Package::detector(
                    "watchpath-detect",
                    &report_with_cache(
                        &[("__test_watchpath", "null")],
                        "\\\"watch_paths\\\": [\\\"$DETECTOR_TEST_WATCH_PATH\\\"]",
                    ),
                )],
                noarch_registrations: Some(registrations(&[(
                    "watchpath-detect",
                    &["__test_watchpath"],
                )])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let registrations = harness.registrations(&[channel]).await;
    // Absent at write time.
    assert!(!cached(&harness, &registrations).await);
    assert!(cached(&harness, &registrations).await);
    // Appears.
    std::fs::write(&watched, "x").unwrap();
    assert!(!cached(&harness, &registrations).await);
    assert!(cached(&harness, &registrations).await);
    // Modification time changes.
    let later = SystemTime::now() + Duration::from_secs(120);
    File::options()
        .write(true)
        .open(&watched)
        .unwrap()
        .set_modified(later)
        .unwrap();
    assert!(!cached(&harness, &registrations).await);
    assert!(cached(&harness, &registrations).await);
    // Disappears.
    std::fs::remove_file(&watched).unwrap();
    assert!(!cached(&harness, &registrations).await);
    assert!(cached(&harness, &registrations).await);
}

#[tokio::test]
async fn huge_lifetimes_are_clamped_and_reboot_is_recorded() {
    let harness = Harness::new();
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![
                    Package::detector(
                        "max-detect",
                        &report_with_cache(
                            &[("__test_max", "null")],
                            "\\\"ttl_seconds\\\": 18446744073709551615",
                        ),
                    ),
                    Package::detector(
                        "year-detect",
                        &report_with_cache(
                            &[("__test_year", "null")],
                            "\\\"ttl_seconds\\\": 31536000",
                        ),
                    ),
                    Package::detector(
                        "reboot-detect",
                        &report_with_cache(
                            &[("__test_reboot", "null")],
                            "\\\"ttl_seconds\\\": \\\"REBOOT\\\"",
                        ),
                    ),
                ],
                noarch_registrations: Some(registrations(&[
                    ("max-detect", &["__test_max"]),
                    ("year-detect", &["__test_year"]),
                    ("reboot-detect", &["__test_reboot"]),
                ])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let registrations = harness.registrations(&[channel]).await;
    let outcome = harness.detect(&registrations).await;
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);

    let entries = harness.cached_results();
    assert_eq!(entries.len(), 3);
    for entry in &entries {
        let detector = entry["key"]["detector"].as_str().unwrap().to_string();
        let lifetime =
            entry["expires_at"].as_u64().unwrap() - entry["written_at"].as_u64().unwrap();
        match detector.as_str() {
            "max-detect" | "year-detect" => {
                assert_eq!(
                    lifetime,
                    MAX_CACHE_LIFETIME.as_secs(),
                    "{detector} was not clamped"
                );
                assert!(entry.get("boot_id").is_none());
            }
            "reboot-detect" => {
                if BootId::current().is_some() {
                    assert!(entry.get("boot_id").is_some(), "no boot id recorded");
                    assert_eq!(lifetime, MAX_CACHE_LIFETIME.as_secs());
                } else {
                    assert!(entry.get("boot_id").is_none());
                    assert_eq!(lifetime, REBOOT_FALLBACK_LIFETIME.as_secs());
                }
            }
            other => panic!("unexpected cache entry for {other}"),
        }
    }
    assert!(from_cache(
        &harness.detect(&registrations).await,
        "__test_reboot"
    ));
}

/// A lifetime above `u64::MAX` is still a nonnegative integer in JSON; the
/// protocol says integer lifetimes are clamped, not rejected.
#[tokio::test]
async fn a_lifetime_beyond_u64_is_clamped_rather_than_rejected() {
    let harness = Harness::new();
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![Package::detector(
                    "beyond-detect",
                    &report_with_cache(
                        &[("__test_beyond", "null")],
                        "\\\"ttl_seconds\\\": 18446744073709551616",
                    ),
                )],
                noarch_registrations: Some(registrations(&[("beyond-detect", &["__test_beyond"])])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let registrations = harness.registrations(&[channel]).await;
    let outcome = harness.detect(&registrations).await;
    assert!(
        outcome.failures.is_empty(),
        "a huge integer lifetime was rejected: {:?}",
        failures(&outcome)
    );
    let entries = harness.cached_results();
    assert_eq!(entries.len(), 1);
    let lifetime =
        entries[0]["expires_at"].as_u64().unwrap() - entries[0]["written_at"].as_u64().unwrap();
    assert_eq!(lifetime, MAX_CACHE_LIFETIME.as_secs());
}

#[tokio::test]
async fn a_corrupted_cache_entry_is_ignored_and_rewritten() {
    let harness = Harness::new();
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![Package::detector(
                    "corrupt-detect",
                    &report(&[("__test_corrupt", &present("1"))]),
                )],
                noarch_registrations: Some(registrations(&[(
                    "corrupt-detect",
                    &["__test_corrupt"],
                )])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let registrations = harness.registrations(&[channel]).await;
    harness.detect(&registrations).await;
    assert!(from_cache(
        &harness.detect(&registrations).await,
        "__test_corrupt"
    ));

    let results = harness.root.join("results");
    let files: Vec<PathBuf> = std::fs::read_dir(&results)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);
    std::fs::write(&files[0], "{ this is not json").unwrap();
    let outcome = harness.detect(&registrations).await;
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    assert!(!from_cache(&outcome, "__test_corrupt"));
    assert!(from_cache(
        &harness.detect(&registrations).await,
        "__test_corrupt"
    ));

    // An entry whose key does not match the file name is ignored too.
    let mut entry: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap();
    entry["key"]["digest"] = serde_json::Value::String("ff".repeat(32));
    std::fs::write(&files[0], serde_json::to_vec(&entry).unwrap()).unwrap();
    assert!(!from_cache(
        &harness.detect(&registrations).await,
        "__test_corrupt"
    ));
}

// ---------------------------------------------------------------------------
// Reports at the edges
// ---------------------------------------------------------------------------

/// A shell snippet that prints a valid report padded to exactly `total`
/// bytes on standard output.
fn padded_report(name: &str, total: usize) -> String {
    let prefix = format!(r#"{{"version": 1, "virtual_packages": {{"{name}": null}}, "padding": ""#);
    let suffix = r#""}"#;
    let padding = total - prefix.len() - suffix.len();
    format!(
        "printf '%s' '{prefix}'\nhead -c {padding} /dev/zero | tr '\\0' a\nprintf '%s' '{suffix}'"
    )
}

#[tokio::test]
async fn reports_must_finish_below_the_combined_output_limit() {
    let harness = Harness::new();
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![
                    Package::detector(
                        "below-detect",
                        &padded_report("__test_below", OUTPUT_LIMIT - 1),
                    ),
                    Package::detector("exact-detect", &padded_report("__test_exact", OUTPUT_LIMIT)),
                    Package::detector(
                        "over-detect",
                        &padded_report("__test_over", OUTPUT_LIMIT + 1),
                    ),
                    // A small report, but standard error pushes the combined
                    // total over the limit.
                    Package::detector(
                        "stderr-detect",
                        &format!(
                            "head -c {} /dev/zero | tr '\\0' e >&2\n{}",
                            OUTPUT_LIMIT - 10,
                            report(&[("__test_stderr", "null")])
                        ),
                    ),
                ],
                noarch_registrations: Some(registrations(&[
                    ("below-detect", &["__test_below"]),
                    ("exact-detect", &["__test_exact"]),
                    ("over-detect", &["__test_over"]),
                    ("stderr-detect", &["__test_stderr"]),
                ])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let registrations = harness.registrations(&[channel]).await;
    let outcome = harness.detect(&registrations).await;
    assert_eq!(
        values(&outcome),
        HashMap::from([("__test_below".to_string(), None)]),
        "failures: {:?}",
        failures(&outcome)
    );
    let failures = failures(&outcome);
    assert_eq!(failures.len(), 3, "{failures:?}");
    for detector in ["exact-detect", "over-detect", "stderr-detect"] {
        assert!(
            matches!(
                failures[detector],
                DetectError::Run(RunError::OutputLimitExceeded { .. })
            ),
            "{detector}: {}",
            failures[detector]
        );
    }
}

#[tokio::test]
async fn report_edge_cases() {
    let harness = Harness::new();
    let sixteen: Vec<String> = (0..16).map(|i| format!("__test_sixteen_{i}")).collect();
    let sixteen_refs: Vec<&str> = sixteen.iter().map(String::as_str).collect();
    let sixteen_report = report(
        &sixteen_refs
            .iter()
            .map(|name| (*name, "null"))
            .collect::<Vec<_>>(),
    );
    let thirty_two_paths = (0..32)
        .map(|i| format!("\\\"/nonexistent/watched/{i}\\\""))
        .collect::<Vec<_>>()
        .join(", ");
    let thirty_two_vars = (0..32)
        .map(|i| format!("\\\"DETECTOR_TEST_VAR_{i}\\\""))
        .collect::<Vec<_>>()
        .join(", ");
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![
                    // `version: 1.0` is a JSON number that is not an integer.
                    Package::detector(
                        "float-detect",
                        r#"echo '{"version": 1.0, "virtual_packages": {"__test_float": null}}'"#,
                    ),
                    Package::detector(
                        "unknown-detect",
                        r#"echo '{"version": 1, "extra": [1, 2], "virtual_packages": {"__test_unknown": {"version": "3", "note": {"deep": true}}}, "cache": {"unknown": 1}, "more": null}'"#,
                    ),
                    Package::detector(
                        "casing-detect",
                        r#"echo '{"version": 1, "virtual_packages": {"__TEST_Casing": {"version": "4"}}}'"#,
                    ),
                    Package::detector(
                        "build-detect",
                        r#"echo '{"version": 1, "virtual_packages": {"__test_build": {"version": "5.1", "build_string": "h1abc_3"}}}'"#,
                    ),
                    Package::detector(
                        "nullbuild-detect",
                        r#"echo '{"version": 1, "virtual_packages": {"__test_nullbuild": {"version": "5.1", "build_string": null}}}'"#,
                    ),
                    Package::detector(
                        "whitespace-detect",
                        "printf '\\n\\n  \\t{\"version\": 1, \"virtual_packages\": {\"__test_whitespace\": null}}  \\n\\n'",
                    ),
                    Package::detector("sixteen-detect", &sixteen_report),
                    Package::detector(
                        "watch32-detect",
                        &report_with_cache(
                            &[("__test_watch32", "null")],
                            &format!(
                                "\\\"watch_paths\\\": [{thirty_two_paths}], \\\"watch_env\\\": [{thirty_two_vars}]"
                            ),
                        ),
                    ),
                    // Executable without the executable bit.
                    Package::library("noexec-detect").file_with_mode(
                        "bin/noexec-detect",
                        &format!("#!/bin/sh\n{}\n", report(&[("__test_noexec", "null")])),
                        0o644,
                    ),
                    // Executable outside the CEP 32 `PATH` directories.
                    Package::library("nobin-detect").file(
                        "libexec/nobin-detect",
                        &format!("#!/bin/sh\n{}\n", report(&[("__test_nobin", "null")])),
                    ),
                    // A valid report followed by a nonzero exit.
                    Package::detector(
                        "exit1-detect",
                        &format!("{}\nexit 1", report(&[("__test_exit1", "null")])),
                    ),
                    // Registered under a key that is not lowercase.
                    Package::detector("case-detect", &report(&[("__test_case", &present("6"))])),
                ],
                noarch_registrations: Some(registrations(&[
                    ("float-detect", &["__test_float"]),
                    ("unknown-detect", &["__test_unknown"]),
                    ("casing-detect", &["__test_casing"]),
                    ("build-detect", &["__test_build"]),
                    ("nullbuild-detect", &["__test_nullbuild"]),
                    ("whitespace-detect", &["__test_whitespace"]),
                    ("sixteen-detect", &sixteen_refs),
                    ("watch32-detect", &["__test_watch32"]),
                    ("noexec-detect", &["__test_noexec"]),
                    ("nobin-detect", &["__test_nobin"]),
                    ("exit1-detect", &["__test_exit1"]),
                    ("Case-Detect", &["__test_case"]),
                    // Registered but not served by the channel.
                    ("ghost-detect", &["__test_ghost"]),
                ])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let output = harness.query(&[channel]).await;
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
    assert_eq!(output.registrations.len(), 13);
    let outcome = harness.detect(&output.registrations).await;

    let values = values(&outcome);
    let mut expected: HashMap<String, Option<String>> = HashMap::from([
        ("__test_unknown".to_string(), Some("3".to_string())),
        ("__test_casing".to_string(), Some("4".to_string())),
        ("__test_build".to_string(), Some("5.1".to_string())),
        ("__test_whitespace".to_string(), None),
        ("__test_watch32".to_string(), None),
        ("__test_case".to_string(), Some("6".to_string())),
    ]);
    for name in &sixteen {
        expected.insert(name.clone(), None);
    }
    assert_eq!(values, expected, "failures: {:?}", failures(&outcome));
    let build = outcome
        .results
        .iter()
        .find(|result| result.name.as_normalized() == "__test_build")
        .unwrap();
    match &build.value {
        DetectedValue::Present(version) => assert_eq!(version.build_string, "h1abc_3"),
        DetectedValue::Absent => panic!("build result absent"),
    }
    let casing = outcome
        .results
        .iter()
        .find(|result| result.name.as_normalized() == "__test_casing")
        .unwrap();
    assert_eq!(
        casing.name.as_source(),
        "__test_casing",
        "the result should carry the registered spelling, not the report's"
    );

    let failures = failures(&outcome);
    let envs = harness.envs();
    let normalize = |message: String| {
        envs.iter().fold(message, |message, env| {
            message.replace(env.to_str().unwrap(), "<prefix>")
        })
    };
    let mut failed: Vec<(&str, String)> = failures
        .iter()
        .map(|(detector, error)| (detector.as_str(), normalize(error.to_string())))
        .collect();
    failed.sort();
    insta::assert_debug_snapshot!(failed, @r#"
    [
        (
            "exit1-detect",
            "the detector exited with exit status: 1",
        ),
        (
            "float-detect",
            "unsupported report version 1.0, expected 1",
        ),
        (
            "ghost-detect",
            "failed to resolve the detector",
        ),
        (
            "nobin-detect",
            "no executable named 'nobin-detect' in <prefix>/bin",
        ),
        (
            "noexec-detect",
            "no executable named 'noexec-detect' in <prefix>/bin",
        ),
        (
            "nullbuild-detect",
            "the report field `virtual_packages.__test_nullbuild.build_string` must be a string",
        ),
    ]
    "#);
    assert!(matches!(
        failures["ghost-detect"],
        DetectError::Environment(EnvironmentError::Solve(_))
    ));
    assert!(matches!(
        failures["noexec-detect"],
        DetectError::Run(RunError::ExecutableNotFound { .. })
    ));

    // The 32-entry watch lists are stored in full.
    let watch32 = harness
        .cached_results()
        .into_iter()
        .find(|entry| entry["key"]["detector"] == "watch32-detect")
        .unwrap();
    assert_eq!(watch32["watch_paths"].as_array().unwrap().len(), 32);
    assert_eq!(watch32["watch_env"].as_array().unwrap().len(), 32);
}

// ---------------------------------------------------------------------------
// Dependencies, activation and link scripts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn link_scripts_are_installed_but_never_run() {
    let mut harness = Harness::new();
    let markers = harness.dir.join("markers");
    std::fs::create_dir_all(&markers).unwrap();
    harness
        .environment
        .insert("DETECTOR_TEST_MARKER_DIR", markers.to_str().unwrap());
    let script = |kind: &str| {
        format!(
            "#!/bin/sh\ntouch \"$DETECTOR_TEST_MARKER_DIR/{kind}\"\ntouch \"${{PREFIX:-.}}/{kind}\"\n"
        )
    };
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![
                    Package::detector(
                        "linked-detect",
                        &report(&[("__test_linked", &present("${LINKED_LIB_ACTIVATED:-0}"))]),
                    )
                    .depends(&["linked-lib"]),
                    Package::library("linked-lib")
                        .file("bin/.linked-lib-post-link.sh", &script("post-link"))
                        .file("bin/.linked-lib-pre-link.sh", &script("pre-link"))
                        .file("bin/.linked-lib-pre-unlink.sh", &script("pre-unlink"))
                        .file(
                            "etc/conda/activate.d/linked-lib.sh",
                            "export LINKED_LIB_ACTIVATED=1\n",
                        ),
                ],
                noarch_registrations: Some(registrations(&[("linked-detect", &["__test_linked"])])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let registrations = harness.registrations(&[channel]).await;
    let outcome = harness.detect(&registrations).await;
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    assert_eq!(values(&outcome)["__test_linked"], Some("1".to_string()));

    let envs = harness.envs();
    assert_eq!(envs.len(), 1);
    assert!(envs[0].join("bin/.linked-lib-post-link.sh").is_file());
    for kind in ["post-link", "pre-link", "pre-unlink"] {
        assert!(!markers.join(kind).exists(), "{kind} script ran");
        assert!(
            !envs[0].join(kind).exists(),
            "{kind} script ran in the prefix"
        );
    }
}

#[tokio::test]
async fn activation_of_one_detector_does_not_leak_into_another() {
    let harness = Harness::new();
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![
                    Package::detector(
                        "activated-detect",
                        &report(&[("__test_activated", &present("${DETECT_LIB_ACTIVATED:-0}"))]),
                    )
                    .depends(&["detect-lib"]),
                    Package::detector(
                        "plain-detect",
                        &report(&[("__test_plain", &present("${DETECT_LIB_ACTIVATED:-0}"))]),
                    ),
                    Package::library("detect-lib").file(
                        "etc/conda/activate.d/detect-lib.sh",
                        "export DETECT_LIB_ACTIVATED=1\n",
                    ),
                ],
                noarch_registrations: Some(registrations(&[
                    ("activated-detect", &["__test_activated"]),
                    ("plain-detect", &["__test_plain"]),
                ])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let registrations = harness.registrations(&[channel]).await;
    for _ in 0..2 {
        let outcome = harness.detect(&registrations).await;
        assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
        assert_eq!(
            values(&outcome),
            HashMap::from([
                ("__test_activated".to_string(), Some("1".to_string())),
                ("__test_plain".to_string(), Some("0".to_string())),
            ])
        );
    }
}

#[tokio::test]
async fn failing_and_slow_activation_scripts_fail_the_detector_only() {
    let harness = Harness::new();
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![
                    Package::detector("good-detect", &report(&[("__test_good", "null")])),
                    Package::detector("broken-detect", &report(&[("__test_broken", "null")]))
                        .depends(&["broken-lib"]),
                    Package::library("broken-lib").file(
                        "etc/conda/activate.d/broken.sh",
                        "echo activation exploded >&2\nexit 7\n",
                    ),
                    Package::detector("slow-detect", &report(&[("__test_slow", "null")]))
                        .depends(&["slow-lib"]),
                    Package::library("slow-lib").file("etc/conda/activate.d/slow.sh", "sleep 30\n"),
                ],
                noarch_registrations: Some(registrations(&[
                    ("good-detect", &["__test_good"]),
                    ("broken-detect", &["__test_broken"]),
                    ("slow-detect", &["__test_slow"]),
                ])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let registrations = harness.registrations(&[channel]).await;
    let mut options = harness.options(&AllowAll);
    options.timeout = Duration::from_secs(2);
    let started = std::time::Instant::now();
    let outcome = detect(&registrations, options).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "activation timeout was not enforced"
    );
    assert_eq!(
        values(&outcome),
        HashMap::from([("__test_good".to_string(), None)])
    );
    let failures = failures(&outcome);
    assert!(
        matches!(
            failures["broken-detect"],
            DetectError::Activation(ActivationError::Failed { .. })
        ),
        "{}",
        failures["broken-detect"]
    );
    let broken = outcome
        .failures
        .iter()
        .find(|failure| failure.detector.as_source() == "broken-detect")
        .unwrap();
    assert!(
        broken
            .stderr
            .as_deref()
            .is_some_and(|stderr| stderr.contains("activation exploded")),
        "{:?}",
        broken.stderr
    );
    assert!(
        matches!(
            failures["slow-detect"],
            DetectError::Activation(ActivationError::TimedOut { .. })
        ),
        "{}",
        failures["slow-detect"]
    );
}

#[tokio::test]
async fn malformed_and_invalid_reports_retain_detector_stderr() {
    let harness = Harness::new();
    for (channel_name, stdout) in [
        ("malformed", "not json"),
        (
            "invalid",
            r#"{"version":1,"virtual_packages":{"__test_unregistered":null}}"#,
        ),
    ] {
        let channel = harness
            .channel(
                channel_name,
                &ChannelSpec {
                    packages: vec![Package::detector(
                        "broken-report-detect",
                        &format!(
                            "printf '%s\\n' 'detector diagnostic' >&2\nprintf '%s' '{stdout}'"
                        ),
                    )],
                    noarch_registrations: Some(registrations(&[(
                        "broken-report-detect",
                        &["__test_report"],
                    )])),
                    ..ChannelSpec::default()
                },
            )
            .await;
        let registrations = harness.registrations(&[channel]).await;
        let outcome = harness.detect(&registrations).await;
        assert!(outcome.results.is_empty());
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(
            outcome.failures[0].stderr.as_deref(),
            Some("detector diagnostic\n"),
            "stderr was lost for the {channel_name} report"
        );
        assert!(harness.cached_results().is_empty());
    }
}

// ---------------------------------------------------------------------------
// Overrides
// ---------------------------------------------------------------------------

#[tokio::test]
async fn overrides_survive_a_failed_detector() {
    let mut harness = Harness::new();
    harness
        .environment
        .insert("CONDA_OVERRIDE_TEST_TNF_A", "4.2=hbuild");
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![Package::detector("tnf-detect", "echo dying >&2\nexit 1")],
                noarch_registrations: Some(registrations(&[(
                    "tnf-detect",
                    &["__test_tnf_a", "__test_tnf_b"],
                )])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let registrations = harness.registrations(&[channel]).await;
    let outcome = harness.detect(&registrations).await;
    assert_eq!(
        values(&outcome),
        HashMap::from([("__test_tnf_a".to_string(), Some("4.2".to_string()))])
    );
    assert_eq!(
        outcome.results[0].source,
        DetectionSource::Override {
            variable: "CONDA_OVERRIDE_TEST_TNF_A".to_string()
        }
    );
    assert_eq!(outcome.failures.len(), 1);
    assert_eq!(
        outcome.failures[0].stderr.as_deref().map(str::trim),
        Some("dying")
    );
    assert!(outcome.skipped.is_empty());
}

#[tokio::test]
async fn override_precedence_holds_even_when_the_detector_disagrees() {
    let mut harness = Harness::new();
    harness
        .environment
        .insert("CONDA_OVERRIDE_TEST_PREC_A", "9.9");
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![Package::detector(
                    "prec-detect",
                    &report(&[
                        ("__test_prec_a", &present("1.0")),
                        ("__test_prec_b", &present("2.0")),
                    ]),
                )],
                noarch_registrations: Some(registrations(&[(
                    "prec-detect",
                    &["__test_prec_a", "__test_prec_b"],
                )])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let registrations = harness.registrations(&[channel]).await;
    let outcome = harness.detect(&registrations).await;
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    assert_eq!(
        values(&outcome),
        HashMap::from([
            ("__test_prec_a".to_string(), Some("9.9".to_string())),
            ("__test_prec_b".to_string(), Some("2.0".to_string())),
        ])
    );
    // Exactly one result per name.
    assert_eq!(outcome.results.len(), 2);
    // The cache stores the detector's own value, not the override.
    let entry = harness.cached_results().pop().unwrap();
    assert_eq!(entry["virtual_packages"]["__test_prec_a"]["version"], "1.0");

    // A cached run must still apply the override.
    let again = harness.detect(&registrations).await;
    assert_eq!(values(&again)["__test_prec_a"], Some("9.9".to_string()));
    assert!(from_cache(&again, "__test_prec_b"));
}

#[tokio::test]
async fn an_override_variable_collision_across_channels_rejects_the_second_registration() {
    let mut harness = Harness::new();
    let a = harness
        .channel(
            "a",
            &ChannelSpec {
                packages: vec![Package::detector(
                    "col-a-detect",
                    &report(&[("__test_col-lide", &present("1"))]),
                )],
                noarch_registrations: Some(registrations(&[(
                    "col-a-detect",
                    &["__test_col-lide"],
                )])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let b = harness
        .channel(
            "b",
            &ChannelSpec {
                packages: vec![Package::detector(
                    "col-b-detect",
                    &report(&[("__test_col_lide", &present("2"))]),
                )],
                noarch_registrations: Some(registrations(&[(
                    "col-b-detect",
                    &["__test_col_lide"],
                )])),
                ..ChannelSpec::default()
            },
        )
        .await;

    let output = harness.query(&[a.clone(), b.clone()]).await;
    assert_eq!(output.registrations.len(), 1);
    assert_eq!(
        output.registrations[0].registration.detector.as_source(),
        "col-a-detect"
    );
    assert_eq!(output.rejected.len(), 1);
    assert_eq!(
        output.rejected[0].registration.detector.as_source(),
        "col-b-detect"
    );
    assert_eq!(
        output.rejected[0].conflict.kind,
        RegistrationConflictKind::OverrideVariable("CONDA_OVERRIDE_TEST_COL_LIDE".to_string())
    );
    assert_eq!(output.rejected[0].conflict.accepted_channel, a.base_url);

    // The reverse order flips the winner.
    let reversed = harness.query(&[b, a]).await;
    assert_eq!(
        reversed.registrations[0].registration.detector.as_source(),
        "col-b-detect"
    );

    // The shared variable overrides the accepted name.
    harness
        .environment
        .insert("CONDA_OVERRIDE_TEST_COL_LIDE", "7");
    let outcome = harness.detect(&output.registrations).await;
    assert_eq!(
        values(&outcome),
        HashMap::from([("__test_col-lide".to_string(), Some("7".to_string()))])
    );
    assert_eq!(outcome.skipped.len(), 1);
    assert_eq!(outcome.skipped[0].reason, SkipReason::NoWantedName);
    assert!(!harness.root.join("envs").exists());
}

// ---------------------------------------------------------------------------
// Channel priority and relations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_base_relation_wins_conflicts_and_supplies_dependencies() {
    let harness = Harness::new();
    let forge = harness
        .channel(
            "forge",
            &ChannelSpec {
                packages: vec![
                    Package::detector(
                        "shared-detect",
                        &report(&[("__test_shared", &present("1.forge"))]),
                    ),
                    // An imposter with a higher version that must never be
                    // picked for bio's registration.
                    Package::detector("bio-detect", &report(&[("__test_bio", &present("9"))]))
                        .version("2.0.0"),
                    Package::library("detect-lib").file(
                        "etc/conda/activate.d/detect-lib.sh",
                        "export DETECT_LIB_ACTIVATED=1\n",
                    ),
                    Package::detector(
                        "forge-only-detect",
                        &report(&[("__test_forge_only", "null")]),
                    ),
                ],
                noarch_registrations: Some(registrations(&[("shared-detect", &["__test_shared"])])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let bio = harness
        .channel(
            "bio",
            &ChannelSpec {
                packages: vec![
                    Package::detector(
                        "shared-detect",
                        &report(&[("__test_shared", &present("1.bio"))]),
                    ),
                    Package::detector(
                        "bio-detect",
                        &report(&[("__test_bio", &present("1.${DETECT_LIB_ACTIVATED:-0}"))]),
                    )
                    .depends(&["detect-lib"]),
                ],
                noarch_registrations: Some(registrations(&[
                    ("shared-detect", &["__test_shared"]),
                    ("bio-detect", &["__test_bio"]),
                    // Registered by bio but only served by forge.
                    ("forge-only-detect", &["__test_forge_only"]),
                ])),
                relations: Some(ChannelRelations {
                    base: Some("../forge".to_string()),
                    overrides: None,
                }),
                ..ChannelSpec::default()
            },
        )
        .await;

    // Only bio is configured; forge is loaded through the relation.
    let output = harness.query(std::slice::from_ref(&bio)).await;
    let accepted: Vec<(String, &str, Vec<String>)> = output
        .registrations
        .iter()
        .map(|registration| {
            (
                registration.origin().to_string(),
                registration.registration.detector.as_source(),
                registration
                    .resolution_channels
                    .iter()
                    .map(|channel| channel.base_url.to_string())
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        accepted,
        [
            (
                forge.base_url.to_string(),
                "shared-detect",
                vec![forge.base_url.to_string()]
            ),
            (
                bio.base_url.to_string(),
                "bio-detect",
                vec![forge.base_url.to_string(), bio.base_url.to_string()]
            ),
            (
                bio.base_url.to_string(),
                "forge-only-detect",
                vec![forge.base_url.to_string(), bio.base_url.to_string()]
            ),
        ]
    );
    assert_eq!(output.rejected.len(), 1);
    assert_eq!(output.rejected[0].channel.base_url, bio.base_url);
    assert_eq!(
        output.rejected[0].registration.detector.as_source(),
        "shared-detect"
    );
    assert_eq!(
        output.rejected[0].conflict.kind,
        RegistrationConflictKind::Name
    );

    let outcome = harness.detect(&output.registrations).await;
    assert_eq!(
        values(&outcome),
        HashMap::from([
            ("__test_shared".to_string(), Some("1.forge".to_string())),
            ("__test_bio".to_string(), Some("1.1".to_string())),
        ]),
        "failures: {:?}",
        failures(&outcome)
    );
    let failures = failures(&outcome);
    assert_eq!(failures.len(), 1);
    assert!(
        matches!(
            failures["forge-only-detect"],
            DetectError::Environment(EnvironmentError::Solve(_))
        ),
        "{}",
        failures["forge-only-detect"]
    );
    // bio's `bio-detect` 1.0.0 was installed, not forge's 2.0.0.
    let installed: Vec<String> = harness
        .envs()
        .iter()
        .flat_map(|env| std::fs::read_dir(env.join("conda-meta")).unwrap())
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .filter(|name| name.starts_with("bio-detect"))
        .collect();
    assert_eq!(installed, ["bio-detect-1.0.0-0.json"]);
}

#[tokio::test]
async fn registrations_in_the_platform_subdir_only_apply_to_that_platform() {
    let harness = Harness::new();
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![Package::detector(
                    "plat-detect",
                    &report(&[("__test_plat", &present("1"))]),
                )],
                noarch_registrations: None,
                platform_registrations: Some(registrations(&[("plat-detect", &["__test_plat"])])),
                ..ChannelSpec::default()
            },
        )
        .await;
    assert!(
        !std::fs::read_to_string(harness.dir.join("channel/noarch/repodata.json"))
            .unwrap()
            .contains("virtual_package_detectors")
    );
    let output = harness.query(std::slice::from_ref(&channel)).await;
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
    assert_eq!(output.registrations.len(), 1);
    let outcome = harness.detect(&output.registrations).await;
    assert_eq!(
        values(&outcome),
        HashMap::from([("__test_plat".to_string(), Some("1".to_string()))]),
        "failures: {:?}",
        failures(&outcome)
    );

    let other = if harness.host == Subdir::Linux64 {
        Subdir::OsxArm64
    } else {
        Subdir::Linux64
    };
    let foreign = harness
        .gateway
        .virtual_package_detectors([channel], [other, Subdir::NoArch])
        .await
        .unwrap();
    assert!(foreign.registrations.is_empty());
    assert!(foreign.warnings.is_empty(), "{:?}", foreign.warnings);
}

// ---------------------------------------------------------------------------
// HTTP channels and sharded repodata
// ---------------------------------------------------------------------------

/// Serves `root` over plain HTTP/1.1 from a background thread; `GET` and
/// `HEAD` of existing files only.
fn serve_static(root: PathBuf) -> Url {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let root = root.clone();
            std::thread::spawn(move || serve_connection(stream, &root));
        }
    });
    Url::parse(&format!("http://127.0.0.1:{}/", address.port())).unwrap()
}

fn serve_connection(stream: TcpStream, root: &Path) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    loop {
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
            return;
        }
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("/").to_string();
        let mut content_length = 0usize;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap_or(0) == 0 {
                return;
            }
            if header.trim().is_empty() {
                break;
            }
            if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
        if content_length > 0 {
            let mut body = vec![0; content_length];
            if reader.read_exact(&mut body).is_err() {
                return;
            }
        }
        let path = target.split('?').next().unwrap_or("/");
        let file = root.join(path.trim_start_matches('/'));
        let (status, body) = match std::fs::read(&file) {
            Ok(bytes) if file.is_file() => ("200 OK", bytes),
            _ => ("404 Not Found", b"not found".to_vec()),
        };
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: keep-alive\r\n\r\n",
            body.len()
        );
        if writer.write_all(head.as_bytes()).is_err() {
            return;
        }
        if method != "HEAD" && writer.write_all(&body).is_err() {
            return;
        }
        if writer.flush().is_err() {
            return;
        }
    }
}

#[tokio::test]
async fn registrations_are_read_from_the_shard_index_of_an_http_channel() {
    let harness = Harness::new();
    let name = "sharded";
    harness
        .channel(
            name,
            &ChannelSpec {
                packages: vec![
                    Package::detector(
                        "shard-detect",
                        &report(&[("__test_shard", &present("${DETECT_LIB_ACTIVATED:-0}"))]),
                    )
                    .depends(&["detect-lib"]),
                    Package::library("detect-lib").file(
                        "etc/conda/activate.d/detect-lib.sh",
                        "export DETECT_LIB_ACTIVATED=1\n",
                    ),
                ],
                noarch_registrations: Some(registrations(&[("shard-detect", &["__test_shard"])])),
                write_shards: true,
                ..ChannelSpec::default()
            },
        )
        .await;
    let channel_dir = harness.dir.join(name);
    assert!(
        channel_dir
            .join("noarch/repodata_shards.msgpack.zst")
            .is_file()
    );
    // Only the sharded form is left, so registrations can only come from the
    // shard index.
    std::fs::remove_file(channel_dir.join("noarch/repodata.json")).unwrap();

    let server = serve_static(harness.dir.clone());
    let channel = Channel::from_url(server.join(&format!("{name}/")).unwrap());
    let output = harness.query(std::slice::from_ref(&channel)).await;
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
    assert_eq!(output.registrations.len(), 1, "{output:?}");
    assert_eq!(output.registrations[0].origin(), &channel.base_url);

    let outcome = harness.detect(&output.registrations).await;
    assert_eq!(
        values(&outcome),
        HashMap::from([("__test_shard".to_string(), Some("1".to_string()))]),
        "failures: {:?}",
        failures(&outcome)
    );
    assert!(from_cache(
        &harness.detect(&output.registrations).await,
        "__test_shard"
    ));
}

#[tokio::test]
async fn an_http_channel_without_shards_is_read_from_repodata_json() {
    let harness = Harness::new();
    let name = "plain";
    harness
        .channel(
            name,
            &ChannelSpec {
                packages: vec![Package::detector(
                    "http-detect",
                    &report(&[("__test_http", &present("1"))]),
                )],
                noarch_registrations: Some(registrations(&[("http-detect", &["__test_http"])])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let server = serve_static(harness.dir.clone());
    let channel = Channel::from_url(server.join(&format!("{name}/")).unwrap());
    let output = harness.query(&[channel]).await;
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
    assert_eq!(output.registrations.len(), 1);
    let outcome = harness.detect(&output.registrations).await;
    assert_eq!(
        values(&outcome),
        HashMap::from([("__test_http".to_string(), Some("1".to_string()))]),
        "failures: {:?}",
        failures(&outcome)
    );
}

// ---------------------------------------------------------------------------
// Cross-process concurrency
// ---------------------------------------------------------------------------

/// The job a worker process runs, written by the driving test.
#[derive(serde::Serialize, serde::Deserialize)]
struct CrossProcessJob {
    channel: PathBuf,
    root: PathBuf,
    package_cache: PathBuf,
    repodata_cache: PathBuf,
}

const CROSS_PROCESS_JOB: &str = "DETECTOR_TEST_XPROC_JOB";

/// Runs one detection for the job named by `DETECTOR_TEST_XPROC_JOB`. Ignored
/// so it only ever runs when `concurrent_processes_install_the_environment_once`
/// starts it.
#[tokio::test]
#[ignore]
async fn cross_process_worker() {
    let Ok(job) = std::env::var(CROSS_PROCESS_JOB) else {
        return;
    };
    let job: CrossProcessJob = serde_json::from_slice(&std::fs::read(job).unwrap()).unwrap();
    let package_cache = PackageCache::new(job.package_cache);
    let gateway = Gateway::builder()
        .with_cache_dir(job.repodata_cache)
        .with_package_cache(package_cache.clone())
        .finish();
    let channel = Channel::try_from_directory(&job.channel).unwrap();
    let host = Subdir::current().unwrap();
    let registrations = gateway
        .virtual_package_detectors([channel], [host, Subdir::NoArch])
        .await
        .unwrap()
        .registrations;
    let environment_root = job.root.join("envs");
    let environment_provider = RattlerEnvironmentProvider::new(EnvironmentOptions {
        gateway: &gateway,
        package_cache: &package_cache,
        download_client: LazyClient::default(),
        root: &environment_root,
        host_platform: host,
        virtual_packages: Vec::new(),
    });
    let outcome = detect(
        &registrations,
        DetectOptions {
            environment_provider: &environment_provider,
            environment: &EnvironmentSnapshot::from_system(),
            root: &job.root,
            host_platform: host,
            target_platform: host,
            timeout: Duration::from_secs(60),
            consent: &AllowAll,
            wanted: WantedNames::All,
            concurrency: 4,
            clock: CacheClock::current(),
        },
    )
    .await
    .unwrap();
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    assert_eq!(
        values(&outcome),
        HashMap::from([("__test_xproc".to_string(), Some("1".to_string()))])
    );
    println!("XPROC_OK");
}

/// Several processes detecting the same registration against the same root
/// and package cache at the same time.
#[tokio::test]
async fn concurrent_processes_install_the_environment_once() {
    const PROCESSES: usize = 6;
    let mut harness = Harness::new();
    let log = harness.dir.join("invocations.log");
    harness
        .environment
        .insert("DETECTOR_TEST_LOG", log.to_str().unwrap());
    harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![
                    Package::detector(
                        "xproc-detect",
                        &format!(
                            "echo run >> \"$DETECTOR_TEST_LOG\"\n{}",
                            report(&[("__test_xproc", &present("${DETECT_LIB_ACTIVATED:-0}"))])
                        ),
                    )
                    .depends(&["detect-lib"]),
                    Package::library("detect-lib").file(
                        "etc/conda/activate.d/detect-lib.sh",
                        "export DETECT_LIB_ACTIVATED=1\n",
                    ),
                ],
                noarch_registrations: Some(registrations(&[("xproc-detect", &["__test_xproc"])])),
                ..ChannelSpec::default()
            },
        )
        .await;

    let exe = std::env::current_exe().unwrap();
    let mut children = Vec::new();
    for index in 0..PROCESSES {
        let job = harness.dir.join(format!("job-{index}.json"));
        std::fs::write(
            &job,
            serde_json::to_vec(&CrossProcessJob {
                channel: harness.dir.join("channel"),
                root: harness.root.clone(),
                package_cache: harness.dir.join("pkgs"),
                repodata_cache: harness.dir.join(format!("repodata-{index}")),
            })
            .unwrap(),
        )
        .unwrap();
        children.push(
            std::process::Command::new(&exe)
                .args([
                    "--ignored",
                    "--exact",
                    "cross_process_worker",
                    "--nocapture",
                ])
                .env_clear()
                .envs(harness.environment.iter())
                .env(CROSS_PROCESS_JOB, &job)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    let mut outputs = Vec::new();
    for child in children {
        outputs.push(child.wait_with_output().unwrap());
    }
    for (index, output) in outputs.iter().enumerate() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && stdout.contains("XPROC_OK"),
            "worker {index} failed with {}:\n{stdout}\n{stderr}",
            output.status
        );
    }
    let envs = harness.envs();
    assert_eq!(envs.len(), 1, "expected one environment, found {envs:?}");
    let runs = count_lines(&log);
    assert!(
        (1..=PROCESSES).contains(&runs),
        "the detector ran {runs} times for {PROCESSES} processes"
    );
}

// ---------------------------------------------------------------------------
// Consent
// ---------------------------------------------------------------------------

struct RecordingConsent {
    envs: PathBuf,
    allow: std::sync::atomic::AtomicBool,
    /// (detector, installed records, whether an environment existed when
    /// asked)
    asked: std::sync::Mutex<Vec<(String, usize, bool)>>,
}

#[async_trait::async_trait]
impl DetectorConsent for RecordingConsent {
    async fn decide(
        &self,
        request: &rattler_virtual_package_detectors::ConsentRequest<'_>,
    ) -> rattler_virtual_package_detectors::Consent {
        self.asked.lock().unwrap().push((
            request.registration.detector.as_source().to_string(),
            request.records.len(),
            self.envs.exists(),
        ));
        if self.allow.load(std::sync::atomic::Ordering::SeqCst) {
            rattler_virtual_package_detectors::Consent::Allow
        } else {
            rattler_virtual_package_detectors::Consent::Deny
        }
    }
}

#[tokio::test]
async fn consent_is_asked_before_installation_and_a_denial_is_not_remembered() {
    let harness = Harness::new();
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![
                    Package::detector(
                        "consent-detect",
                        &report(&[("__test_consent", &present("1"))]),
                    )
                    .depends(&["detect-lib"]),
                    Package::library("detect-lib"),
                ],
                noarch_registrations: Some(registrations(&[(
                    "consent-detect",
                    &["__test_consent"],
                )])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let registrations = harness.registrations(&[channel]).await;
    let consent = RecordingConsent {
        envs: harness.root.join("envs"),
        allow: std::sync::atomic::AtomicBool::new(false),
        asked: std::sync::Mutex::new(Vec::new()),
    };

    let denied = detect(&registrations, harness.options(&consent))
        .await
        .unwrap();
    assert!(denied.results.is_empty());
    assert_eq!(denied.skipped[0].reason, SkipReason::ConsentDenied);
    assert!(!harness.root.join("envs").exists());
    assert!(!harness.root.join("results").exists());

    consent
        .allow
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let allowed = detect(&registrations, harness.options(&consent))
        .await
        .unwrap();
    assert_eq!(values(&allowed)["__test_consent"], Some("1".to_string()));
    assert!(!from_cache(&allowed, "__test_consent"));

    let cached = detect(&registrations, harness.options(&consent))
        .await
        .unwrap();
    assert!(from_cache(&cached, "__test_consent"));

    let asked = consent.asked.lock().unwrap().clone();
    assert_eq!(
        asked,
        [
            ("consent-detect".to_string(), 2, false),
            ("consent-detect".to_string(), 2, false),
            ("consent-detect".to_string(), 2, true),
        ]
    );
}

// ---------------------------------------------------------------------------
// More channel interplay
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_same_detector_name_in_two_channels_runs_both() {
    let harness = Harness::new();
    let a = harness
        .channel(
            "a",
            &ChannelSpec {
                packages: vec![Package::detector(
                    "x-detect",
                    &report(&[("__test_xa", &present("1"))]),
                )],
                noarch_registrations: Some(registrations(&[("x-detect", &["__test_xa"])])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let b = harness
        .channel(
            "b",
            &ChannelSpec {
                packages: vec![Package::detector(
                    "x-detect",
                    &report(&[("__test_xb", &present("2"))]),
                )],
                noarch_registrations: Some(registrations(&[("x-detect", &["__test_xb"])])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let output = harness.query(&[a.clone(), b.clone()]).await;
    assert_eq!(output.registrations.len(), 2);
    assert!(output.rejected.is_empty());
    let outcome = harness.detect(&output.registrations).await;
    assert_eq!(
        values(&outcome),
        HashMap::from([
            ("__test_xa".to_string(), Some("1".to_string())),
            ("__test_xb".to_string(), Some("2".to_string())),
        ]),
        "failures: {:?}",
        failures(&outcome)
    );
    assert_ne!(
        digest_of(&outcome, "__test_xa"),
        digest_of(&outcome, "__test_xb")
    );
    assert_eq!(harness.envs().len(), 2);
    let again = harness.detect(&output.registrations).await;
    assert!(from_cache(&again, "__test_xa") && from_cache(&again, "__test_xb"));
    assert_eq!(values(&again), values(&outcome));
}

#[tokio::test]
async fn an_overrides_relation_gives_the_declaring_channel_priority() {
    let harness = Harness::new();
    let forge = harness
        .channel(
            "forge",
            &ChannelSpec {
                packages: vec![Package::detector(
                    "shared-detect",
                    &report(&[("__test_shared", &present("1.forge"))]),
                )],
                noarch_registrations: Some(registrations(&[("shared-detect", &["__test_shared"])])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let top = harness
        .channel(
            "top",
            &ChannelSpec {
                packages: vec![Package::detector(
                    "shared-detect",
                    &report(&[("__test_shared", &present("1.top"))]),
                )],
                noarch_registrations: Some(registrations(&[("shared-detect", &["__test_shared"])])),
                relations: Some(ChannelRelations {
                    base: None,
                    overrides: Some("../forge".to_string()),
                }),
                ..ChannelSpec::default()
            },
        )
        .await;
    let output = harness.query(std::slice::from_ref(&top)).await;
    assert_eq!(output.registrations.len(), 1);
    assert_eq!(output.registrations[0].origin(), &top.base_url);
    assert_eq!(
        output.registrations[0]
            .resolution_channels
            .iter()
            .map(|channel| channel.base_url.clone())
            .collect::<Vec<_>>(),
        [top.base_url.clone(), forge.base_url.clone()]
    );
    assert_eq!(output.rejected.len(), 1);
    assert_eq!(output.rejected[0].channel.base_url, forge.base_url);
    let outcome = harness.detect(&output.registrations).await;
    assert_eq!(
        values(&outcome),
        HashMap::from([("__test_shared".to_string(), Some("1.top".to_string()))]),
        "failures: {:?}",
        failures(&outcome)
    );
}

#[tokio::test]
async fn overrides_apply_even_when_the_target_is_not_the_host() {
    let mut harness = Harness::new();
    harness.environment.insert("CONDA_OVERRIDE_TEST_FT_A", "3");
    let channel = harness
        .channel(
            "channel",
            &ChannelSpec {
                packages: vec![Package::detector(
                    "ft-detect",
                    &report(&[("__test_ft_a", "null"), ("__test_ft_b", "null")]),
                )],
                noarch_registrations: Some(registrations(&[(
                    "ft-detect",
                    &["__test_ft_a", "__test_ft_b"],
                )])),
                ..ChannelSpec::default()
            },
        )
        .await;
    let registrations = harness.registrations(&[channel]).await;
    let mut options = harness.options(&AllowAll);
    options.target_platform = if harness.host == Subdir::Linux64 {
        Subdir::OsxArm64
    } else {
        Subdir::Linux64
    };
    let outcome = detect(&registrations, options).await.unwrap();
    assert_eq!(
        values(&outcome),
        HashMap::from([("__test_ft_a".to_string(), Some("3".to_string()))])
    );
    assert_eq!(
        outcome.skipped[0].reason,
        SkipReason::TargetIsNotHost {
            override_variables: vec!["CONDA_OVERRIDE_TEST_FT_B".to_string()]
        }
    );
    assert!(!harness.root.exists());
}
