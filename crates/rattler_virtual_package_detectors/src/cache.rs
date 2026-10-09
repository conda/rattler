//! Caching detector results between solves.
//!
//! An entry is keyed on the registration identity, the registered names and
//! the environment digest, and it always expires: after the lifetime the
//! report asked for, clamped to the protocol's maximum, at the next reboot for
//! `"REBOOT"` results, and early when a watched path or variable changes.
//! Entries are JSON files written atomically, so concurrent readers never see
//! a partial file.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use indexmap::IndexMap;
use rattler_boot_id::BootId;
use rattler_conda_types::{ChannelUrl, PackageName};
use rattler_digest::{Sha256, Sha256Hash, compute_bytes_digest};
use rattler_shell::environment::EnvironmentSnapshot;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::AsyncWriteExt;

use crate::{
    limits::{DEFAULT_CACHE_LIFETIME, MAX_CACHE_LIFETIME, REBOOT_FALLBACK_LIFETIME},
    report::{CacheLifetime, DetectedVersion, DetectorReport},
};

/// What identifies a cached result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheKey {
    /// The registration's origin.
    pub origin: ChannelUrl,
    /// The detector.
    pub detector: PackageName,
    /// The registered virtual package names, sorted.
    pub names: Vec<PackageName>,
    /// The environment digest, as lowercase hexadecimal.
    pub digest: String,
}

impl CacheKey {
    /// Builds the key for a detector environment.
    pub fn new(
        origin: ChannelUrl,
        detector: PackageName,
        names: impl IntoIterator<Item = PackageName>,
        digest: &Sha256Hash,
    ) -> Self {
        let mut names: Vec<PackageName> = names.into_iter().collect();
        names.sort();
        Self {
            origin,
            detector,
            names,
            digest: hex::encode(digest),
        }
    }

    /// The normalized detector name followed by the full hash of the key.
    pub fn file_name(&self) -> String {
        let mut material = String::new();
        material.push_str(self.origin.as_str());
        material.push('\0');
        material.push_str(self.detector.as_normalized());
        material.push('\0');
        for name in &self.names {
            material.push_str(name.as_normalized());
            material.push(',');
        }
        material.push('\0');
        material.push_str(&self.digest);
        format!(
            "{}-{}.json",
            self.detector.as_normalized(),
            hex::encode(compute_bytes_digest::<Sha256>(material))
        )
    }
}

/// The observed state of a watched path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathState {
    /// The path does not exist or cannot be accessed.
    Absent,
    /// The path exists with this modification time.
    Present {
        /// Seconds since the Unix epoch.
        modified_secs: u64,
        /// Nanoseconds within that second.
        modified_nanos: u32,
    },
}

impl PathState {
    async fn observe(path: &Path) -> Self {
        let Ok(metadata) = tokio::fs::metadata(path).await else {
            return Self::Absent;
        };
        let Ok(modified) = metadata.modified() else {
            return Self::Absent;
        };
        let since_epoch = modified.duration_since(UNIX_EPOCH).unwrap_or_default();
        Self::Present {
            modified_secs: since_epoch.as_secs(),
            modified_nanos: since_epoch.subsec_nanos(),
        }
    }
}

/// A watched path and its state when the entry was written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchedPath {
    /// The absolute path.
    pub path: PathBuf,
    /// Its state at write time.
    pub state: PathState,
}

/// A watched environment variable and a fingerprint of its value when the
/// entry was written. Only the fingerprint is stored: a detector may watch
/// variables that hold credentials, and the cache file must not leak them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchedVariable {
    /// The variable's name.
    pub name: String,
    /// The SHA-256 of its value at write time, as lowercase hexadecimal, or
    /// `None` if unset.
    pub value_digest: Option<String>,
}

impl WatchedVariable {
    fn observe(name: &str, env: &EnvironmentSnapshot) -> Self {
        let value = env.get(name);
        Self {
            name: name.to_string(),
            value_digest: value
                .map(|value| hex::encode(compute_bytes_digest::<Sha256>(value.as_encoded_bytes()))),
        }
    }
}

/// A cached result as stored on disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedResult {
    /// The key the entry was written for.
    pub key: CacheKey,
    /// When the entry was written, in seconds since the Unix epoch.
    pub written_at: u64,
    /// When the entry expires, in seconds since the Unix epoch.
    pub expires_at: u64,
    /// The boot session the entry is valid in, for `"REBOOT"` results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<BootId>,
    /// The watched paths and their states at write time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watch_paths: Vec<WatchedPath>,
    /// The watched variables and their values at write time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watch_env: Vec<WatchedVariable>,
    /// The detector's results.
    pub virtual_packages: IndexMap<PackageName, Option<DetectedVersion>>,
    /// Nonempty standard error from the successful invocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
}

#[derive(Serialize)]
struct BorrowedCachedResult<'a> {
    key: &'a CacheKey,
    written_at: u64,
    expires_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    boot_id: Option<BootId>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    watch_paths: Vec<WatchedPath>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    watch_env: Vec<WatchedVariable>,
    virtual_packages: &'a IndexMap<PackageName, Option<DetectedVersion>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stderr: Option<&'a str>,
}

/// The time and boot session against which entries are validated. Passed
/// explicitly so the expiry rules can be tested deterministically.
#[derive(Clone, Debug)]
pub struct CacheClock {
    /// The current time in seconds since the Unix epoch.
    pub now: u64,
    /// The current boot session, if it can be observed on this host.
    pub boot_id: Option<BootId>,
}

impl CacheClock {
    /// The real clock and boot session.
    pub fn current() -> Self {
        Self {
            now: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs()),
            boot_id: BootId::current(),
        }
    }
}

/// Why an entry could not be written.
#[derive(Debug, Error)]
pub enum CacheError {
    /// The entry could not be serialized.
    #[error("failed to serialize the cached result")]
    Serialize(#[from] serde_json::Error),

    /// The entry could not be written.
    #[error("failed to write the cached result to {}", path.display())]
    Io {
        /// The entry's path.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
}

/// The on-disk cache of detector results.
#[derive(Clone, Debug)]
pub struct ResultCache {
    root: PathBuf,
}

impl ResultCache {
    /// A cache whose entries live directly in `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The directory the entries live in.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path_for(&self, key: &CacheKey) -> PathBuf {
        self.root.join(key.file_name())
    }

    /// Returns the valid entry for `key`, if there is one.
    ///
    /// An unreadable or malformed entry counts as absent.
    pub async fn read(
        &self,
        key: &CacheKey,
        clock: &CacheClock,
        environment: &EnvironmentSnapshot,
    ) -> Option<CachedResult> {
        let path = self.path_for(key);
        let bytes = fs_err::tokio::read(&path).await.ok()?;
        let entry: CachedResult = serde_json::from_slice(&bytes).ok()?;
        if &entry.key != key {
            tracing::debug!(path = %path.display(), "ignoring cached result for a different key");
            return None;
        }
        match entry.is_valid(clock, environment).await {
            Ok(()) => Some(entry),
            Err(reason) => {
                tracing::debug!(path = %path.display(), reason, "cached result expired");
                None
            }
        }
    }

    /// Stores `report`'s results and diagnostics for `key`, honoring its cache hints.
    ///
    /// Writes nothing when the report asked not to be reused.
    pub async fn write(
        &self,
        key: &CacheKey,
        report: &DetectorReport,
        stderr: Option<&str>,
        clock: &CacheClock,
        environment: &EnvironmentSnapshot,
    ) -> Result<(), CacheError> {
        let Some(lifetime) = lifetime(report.cache.ttl, clock.boot_id.is_some()) else {
            return Ok(());
        };
        let boot_id = match report.cache.ttl {
            Some(CacheLifetime::Reboot) => clock.boot_id.clone(),
            _ => None,
        };
        let mut watch_paths = Vec::with_capacity(report.cache.watch_paths.len());
        for path in &report.cache.watch_paths {
            watch_paths.push(WatchedPath {
                path: path.clone(),
                state: PathState::observe(path).await,
            });
        }
        let entry = BorrowedCachedResult {
            key,
            written_at: clock.now,
            expires_at: clock.now.saturating_add(lifetime.as_secs()),
            boot_id,
            watch_paths,
            watch_env: report
                .cache
                .watch_env
                .iter()
                .map(|name| WatchedVariable::observe(name, environment))
                .collect(),
            virtual_packages: &report.virtual_packages,
            stderr: stderr.filter(|stderr| !stderr.is_empty()),
        };

        let path = self.path_for(key);
        let io_error = |source| CacheError::Io {
            path: path.clone(),
            source,
        };
        fs_err::tokio::create_dir_all(&self.root)
            .await
            .map_err(io_error)?;
        let bytes = serde_json::to_vec_pretty(&entry)?;
        // Written next to the entry and renamed into place, so a reader never
        // sees a partial file.
        let (file, temporary) = tempfile::NamedTempFile::new_in(&self.root)
            .map_err(io_error)?
            .into_parts();
        let mut file = tokio::fs::File::from_std(file);
        file.write_all(&bytes).await.map_err(io_error)?;
        file.flush().await.map_err(io_error)?;
        drop(file);
        fs_err::tokio::rename(&temporary, &path)
            .await
            .map_err(io_error)?;
        Ok(())
    }

    /// Removes every entry.
    pub async fn clear(&self) -> std::io::Result<()> {
        match fs_err::tokio::remove_dir_all(&self.root).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }
}

impl CachedResult {
    /// Checks the entry against `clock`, returning why it is no longer valid.
    pub async fn is_valid(
        &self,
        clock: &CacheClock,
        environment: &EnvironmentSnapshot,
    ) -> Result<(), &'static str> {
        if clock.now >= self.expires_at {
            return Err("lifetime elapsed");
        }
        if let Some(boot_id) = &self.boot_id {
            match &clock.boot_id {
                Some(current) if boot_id.matches(current) => {}
                _ => return Err("boot session changed"),
            }
        }
        for watched in &self.watch_paths {
            if PathState::observe(&watched.path).await != watched.state {
                return Err("watched path changed");
            }
        }
        for watched in &self.watch_env {
            if WatchedVariable::observe(&watched.name, environment) != *watched {
                return Err("watched variable changed");
            }
        }
        Ok(())
    }
}

/// The lifetime an entry gets for the requested `ttl`, or `None` when the
/// result must not be reused.
fn lifetime(ttl: Option<CacheLifetime>, boot_observable: bool) -> Option<Duration> {
    match ttl {
        None => Some(DEFAULT_CACHE_LIFETIME),
        Some(CacheLifetime::Seconds(0)) => None,
        Some(CacheLifetime::Seconds(seconds)) => {
            Some(Duration::from_secs(seconds).min(MAX_CACHE_LIFETIME))
        }
        Some(CacheLifetime::Reboot) if boot_observable => Some(MAX_CACHE_LIFETIME),
        Some(CacheLifetime::Reboot) => Some(REBOOT_FALLBACK_LIFETIME),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    use std::{str::FromStr, sync::Arc};

    use rattler_conda_types::Version;
    use url::Url;

    use super::*;
    use crate::report::CacheHints;

    #[cfg(unix)]
    #[test]
    fn watched_native_values_distinguish_bytes_empty_and_unset() {
        let mut environment = EnvironmentSnapshot::default();
        let unset = WatchedVariable::observe("NATIVE", &environment);
        environment.insert("NATIVE", "");
        let empty = WatchedVariable::observe("NATIVE", &environment);
        assert_ne!(unset.value_digest, empty.value_digest);
        environment.insert("NATIVE", OsString::from_vec(vec![0xff]));
        let first = WatchedVariable::observe("NATIVE", &environment);
        environment.insert("NATIVE", OsString::from_vec(vec![0xfe]));
        let second = WatchedVariable::observe("NATIVE", &environment);
        assert_ne!(first.value_digest, second.value_digest);
        assert_ne!(first.value_digest, empty.value_digest);
    }

    fn key() -> CacheKey {
        CacheKey::new(
            ChannelUrl::from(Url::parse("https://conda.anaconda.org/conda-forge").unwrap()),
            PackageName::try_from("mpi-detect").unwrap(),
            [
                PackageName::try_from("__b").unwrap(),
                PackageName::try_from("__a").unwrap(),
            ],
            &Sha256Hash::default(),
        )
    }

    fn report(cache: CacheHints) -> DetectorReport {
        DetectorReport {
            virtual_packages: IndexMap::from([(
                PackageName::try_from("__a").unwrap(),
                Some(DetectedVersion {
                    version: Version::from_str("1.0").unwrap(),
                    build_string: "0".to_string(),
                }),
            )]),
            cache,
        }
    }

    fn clock(now: u64) -> CacheClock {
        CacheClock {
            now,
            boot_id: Some(BootId::Uuid("boot-1".to_string())),
        }
    }

    fn env() -> EnvironmentSnapshot {
        [("WATCHED", "one")].into_iter().collect()
    }

    #[test]
    fn key_sorts_names_and_hashes_the_file_name() {
        let key = key();
        assert_eq!(
            key.names
                .iter()
                .map(PackageName::as_normalized)
                .collect::<Vec<_>>(),
            ["__a", "__b"]
        );
        assert!(key.file_name().ends_with(".json"));
        let mut other = key.clone();
        other.digest = "ff".repeat(32);
        assert_ne!(key.file_name(), other.file_name());
    }

    #[test]
    fn lifetimes_follow_the_protocol() {
        assert_eq!(lifetime(None, true), Some(DEFAULT_CACHE_LIFETIME));
        assert_eq!(lifetime(Some(CacheLifetime::Seconds(0)), true), None);
        assert_eq!(
            lifetime(Some(CacheLifetime::Seconds(10)), true),
            Some(Duration::from_secs(10))
        );
        assert_eq!(
            lifetime(Some(CacheLifetime::Seconds(u64::MAX)), true),
            Some(MAX_CACHE_LIFETIME)
        );
        assert_eq!(
            lifetime(Some(CacheLifetime::Reboot), true),
            Some(MAX_CACHE_LIFETIME)
        );
        assert_eq!(
            lifetime(Some(CacheLifetime::Reboot), false),
            Some(REBOOT_FALLBACK_LIFETIME)
        );
    }

    #[tokio::test]
    async fn round_trip_and_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResultCache::new(dir.path().join("results"));
        let key = key();
        cache
            .write(
                &key,
                &report(CacheHints {
                    ttl: Some(CacheLifetime::Seconds(100)),
                    watch_paths: Vec::new(),
                    watch_env: vec!["WATCHED".to_string()],
                }),
                Some("hardware probe warning\n"),
                &clock(1_000),
                &env(),
            )
            .await
            .unwrap();
        let written = cache.read(&key, &clock(1_000), &env()).await.unwrap();
        assert_eq!(written.stderr.as_deref(), Some("hardware probe warning\n"));
        assert_eq!(written.expires_at, 1_100);

        assert_eq!(
            cache.read(&key, &clock(1_050), &env()).await,
            Some(written.clone())
        );
        assert_eq!(cache.read(&key, &clock(1_100), &env()).await, None);

        let mut changed = env();
        changed.insert("WATCHED", "two");
        assert_eq!(cache.read(&key, &clock(1_050), &changed).await, None);
        changed.remove("WATCHED");
        assert_eq!(cache.read(&key, &clock(1_050), &changed).await, None);

        cache.clear().await.unwrap();
        assert_eq!(cache.read(&key, &clock(1_050), &env()).await, None);
        cache.clear().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_publications_leave_one_complete_report() {
        const WRITERS: usize = 64;
        let dir = tempfile::tempdir().unwrap();
        let cache = ResultCache::new(dir.path());
        let key = key();
        let barrier = Arc::new(tokio::sync::Barrier::new(WRITERS));
        let mut writers = tokio::task::JoinSet::new();
        for index in 0..WRITERS {
            let cache = cache.clone();
            let key = key.clone();
            let barrier = barrier.clone();
            writers.spawn(async move {
                let mut report = report(CacheHints::default());
                report
                    .virtual_packages
                    .values_mut()
                    .next()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .build_string = index.to_string().repeat(index + 1);
                barrier.wait().await;
                let stderr = format!("publication {index}");
                cache
                    .write(&key, &report, Some(&stderr), &clock(1), &env())
                    .await?;
                Ok::<_, CacheError>((report.virtual_packages, stderr))
            });
        }
        let mut publications = Vec::new();
        while let Some(result) = writers.join_next().await {
            publications.push(result.unwrap());
        }
        let publications = publications
            .into_iter()
            .map(|result| result.expect("a concurrent publication failed"))
            .collect::<Vec<_>>();
        let cached = cache
            .read(&key, &clock(2), &env())
            .await
            .expect("concurrent publications left an unreadable cache entry");
        assert!(
            publications.contains(&(cached.virtual_packages, cached.stderr.unwrap())),
            "the cached entry combines data from different publications"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn watched_windows_variables_ignore_name_case_but_not_value_changes() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResultCache::new(dir.path());
        let key = key();
        let original: EnvironmentSnapshot = [("Path", "original")].into_iter().collect();
        cache
            .write(
                &key,
                &report(CacheHints {
                    watch_env: vec!["PATH".to_string()],
                    ..CacheHints::default()
                }),
                None,
                &clock(1),
                &original,
            )
            .await
            .unwrap();
        let stored = cache.read(&key, &clock(1), &original).await.unwrap();
        let mut changed: EnvironmentSnapshot = [("Path", "changed")].into_iter().collect();
        assert_eq!(cache.read(&key, &clock(2), &changed).await, None);

        changed = [("pAtH", "original")].into_iter().collect();
        assert_eq!(cache.read(&key, &clock(2), &changed).await, Some(stored));
        changed.remove("PATH");
        assert_eq!(cache.read(&key, &clock(2), &changed).await, None);
    }

    #[tokio::test]
    async fn ttl_zero_is_never_stored() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResultCache::new(dir.path());
        cache
            .write(
                &key(),
                &report(CacheHints {
                    ttl: Some(CacheLifetime::Seconds(0)),
                    ..CacheHints::default()
                }),
                Some("uncached diagnostics"),
                &clock(1),
                &env(),
            )
            .await
            .unwrap();
        assert!(cache.read(&key(), &clock(1), &env()).await.is_none());
    }

    #[tokio::test]
    async fn reboot_entries_expire_with_the_boot_session() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResultCache::new(dir.path());
        cache
            .write(
                &key(),
                &report(CacheHints {
                    ttl: Some(CacheLifetime::Reboot),
                    ..CacheHints::default()
                }),
                None,
                &clock(1),
                &env(),
            )
            .await
            .unwrap();
        assert!(cache.read(&key(), &clock(2), &env()).await.is_some());
        let mut rebooted = clock(2);
        rebooted.boot_id = Some(BootId::Uuid("boot-2".to_string()));
        assert!(cache.read(&key(), &rebooted, &env()).await.is_none());
        let mut unknown = clock(2);
        unknown.boot_id = None;
        assert!(cache.read(&key(), &unknown, &env()).await.is_none());
    }

    #[tokio::test]
    async fn watched_paths_expire_on_change() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResultCache::new(dir.path().join("results"));
        let watched = dir.path().join("watched");
        let hints = CacheHints {
            ttl: None,
            watch_paths: vec![watched.clone()],
            watch_env: Vec::new(),
        };
        // Absent at write time: appearing expires the entry.
        cache
            .write(&key(), &report(hints.clone()), None, &clock(1), &env())
            .await
            .unwrap();
        assert!(cache.read(&key(), &clock(2), &env()).await.is_some());
        std::fs::write(&watched, "x").unwrap();
        assert!(cache.read(&key(), &clock(2), &env()).await.is_none());

        // Present at write time: disappearing expires the entry.
        cache
            .write(&key(), &report(hints), None, &clock(3), &env())
            .await
            .unwrap();
        assert!(cache.read(&key(), &clock(4), &env()).await.is_some());
        std::fs::remove_file(&watched).unwrap();
        assert!(cache.read(&key(), &clock(4), &env()).await.is_none());
    }

    #[tokio::test]
    async fn old_entries_without_diagnostics_remain_valid() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResultCache::new(dir.path());
        let key = key();
        let old_entry = serde_json::json!({
            "key": key,
            "written_at": 1,
            "expires_at": 100,
            "virtual_packages": { "__a": null, "__b": null }
        });
        std::fs::write(
            dir.path().join(key.file_name()),
            serde_json::to_vec(&old_entry).unwrap(),
        )
        .unwrap();
        let cached = cache.read(&key, &clock(2), &env()).await.unwrap();
        assert!(cached.stderr.is_none());
        assert_eq!(cached.virtual_packages.len(), 2);
        assert!(
            !serde_json::to_value(cached)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("stderr")
        );
    }

    #[tokio::test]
    async fn empty_diagnostics_are_not_stored() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResultCache::new(dir.path());
        let key = key();
        cache
            .write(
                &key,
                &report(CacheHints::default()),
                Some(""),
                &clock(1),
                &env(),
            )
            .await
            .unwrap();
        let cached = cache.read(&key, &clock(2), &env()).await.unwrap();
        assert!(cached.stderr.is_none());
    }

    #[tokio::test]
    async fn entries_for_other_keys_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResultCache::new(dir.path());
        let key = key();
        cache
            .write(
                &key,
                &report(CacheHints::default()),
                None,
                &clock(1),
                &env(),
            )
            .await
            .unwrap();
        // Rewrite the entry on disk under the same file name with a different
        // key, as a hash collision would: the stored key is verified.
        let path = dir.path().join(key.file_name());
        let mut entry: CachedResult =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        entry.key.digest = "ff".repeat(32);
        std::fs::write(&path, serde_json::to_vec(&entry).unwrap()).unwrap();
        assert!(cache.read(&key, &clock(2), &env()).await.is_none());
    }
}
