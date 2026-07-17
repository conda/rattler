//! Persistent overlay state management.
//!
//! Tracks whiteouts (deleted files) and environment identity in a JSON state
//! file. The state file is written atomically (write-tmp → fsync → rename) on
//! every mutation for crash safety.

use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) const STATE_FILENAME: &str = ".rattler_vfs_state.json";
pub(crate) const STATE_TMP_FILENAME: &str = ".rattler_vfs_state.tmp";
pub(crate) const STATE_LOCK_FILENAME: &str = ".rattler_vfs_state.lock";

#[derive(Debug)]
pub enum OverlayError {
    Io(std::io::Error),
    Json(serde_json::Error),
    /// Env hash mismatch.  The `lock` field carries the directory lock.
    /// `create_overlay` returns this as a structured `MountError` so the
    /// caller can decide whether to wipe — the overlay may contain user work.
    EnvHashMismatch {
        expected: String,
        found: String,
        lock: fs::File,
    },
    /// Transport mismatch.  The `lock` field carries the directory lock.
    TransportMismatch {
        expected: String,
        found: String,
        lock: fs::File,
    },
    /// State file version mismatch.  The `lock` field carries the directory lock.
    VersionMismatch {
        expected: u32,
        found: u32,
        lock: fs::File,
    },
}

impl std::fmt::Display for OverlayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "overlay I/O error: {e}"),
            Self::Json(e) => write!(f, "overlay state file error: {e}"),
            Self::EnvHashMismatch {
                expected, found, ..
            } => {
                write!(
                    f,
                    "overlay environment mismatch: expected {expected}, found {found}"
                )
            }
            Self::TransportMismatch {
                expected, found, ..
            } => {
                write!(
                    f,
                    "overlay transport mismatch: expected {expected}, found {found}"
                )
            }
            Self::VersionMismatch {
                expected, found, ..
            } => {
                write!(
                    f,
                    "overlay state version mismatch: expected {expected}, found {found}"
                )
            }
        }
    }
}

impl From<std::io::Error> for OverlayError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for OverlayError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// Current state file format version. Bump when the format changes
/// incompatibly. On load, if the version doesn't match, the overlay is
/// considered stale and should be wiped.
const STATE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct StateFile {
    /// Format version — reject overlays from incompatible versions.
    #[serde(default)]
    version: u32,
    env_hash: String,
    /// Transport that created this overlay (e.g. "fuse", "nfs").
    /// Used to detect incompatible overlay/transport combinations.
    #[serde(default)]
    transport: String,
    whiteouts: Vec<PathBuf>,
    #[serde(default)]
    opaque_dirs: Vec<PathBuf>,
}

/// Read the environment hash recorded in an overlay directory's state file,
/// without taking the directory lock or creating anything.
///
/// Returns `None` when there is no overlay yet (a fresh directory) or the state
/// file is missing/unreadable/unparseable/from an incompatible version. This is
/// a cheap, side-effect-free probe for callers that want to detect an
/// environment change *before* mounting — e.g. to warn the user that a
/// persistent overlay will be reused for a different environment.
pub fn recorded_env_hash(overlay_dir: &Path) -> Option<String> {
    let content = fs::read_to_string(overlay_dir.join(STATE_FILENAME)).ok()?;
    let state: StateFile = serde_json::from_str(&content).ok()?;
    if state.version != STATE_VERSION {
        return None;
    }
    Some(state.env_hash)
}

/// Persistent overlay state: whiteouts, environment identity, and transport type.
///
/// Holds an exclusive file lock on `.rattler_vfs_state.lock` for its entire
/// lifetime, preventing concurrent access to the same overlay directory.
/// The lock is released automatically on drop (or on process crash, since
/// advisory file locks are released by the OS).
pub struct OverlayState {
    dir: PathBuf,
    pub(crate) whiteouts: HashSet<PathBuf>,
    pub(crate) opaque_dirs: HashSet<PathBuf>,
    state_path: PathBuf,
    env_hash: String,
    transport: String,
    /// Exclusive file lock held for the lifetime of this state.  Dropping
    /// the `File` releases the lock.
    _lock: fs::File,
}

impl std::fmt::Debug for OverlayState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverlayState")
            .field("dir", &self.dir)
            .field("env_hash", &self.env_hash)
            .field("transport", &self.transport)
            .field("whiteouts", &self.whiteouts.len())
            .field("opaque_dirs", &self.opaque_dirs.len())
            .finish()
    }
}

impl OverlayState {
    /// Acquire the overlay directory lock without loading state.
    ///
    /// Returns the lock file handle. Use with [`Self::load_with_lock`] when the
    /// caller needs to hold the lock across a wipe-and-retry cycle.
    pub fn acquire_lock(dir: &Path) -> Result<fs::File, OverlayError> {
        fs::create_dir_all(dir)?;
        let lock_path = dir.join(STATE_LOCK_FILENAME);
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        if lock.try_lock().is_err() {
            tracing::warn!(
                "overlay state lock at {} is held by another process; waiting up to 15s",
                lock_path.display(),
            );
            // Bounded wait rather than blocking forever: a stuck holder would
            // otherwise surface only as an opaque readiness timeout upstream.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            loop {
                std::thread::sleep(std::time::Duration::from_millis(200));
                if lock.try_lock().is_ok() {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    return Err(OverlayError::Io(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        format!(
                            "overlay state lock at {} is still held after 15s; is another \
                             mount of this environment already running?",
                            lock_path.display()
                        ),
                    )));
                }
            }
        }
        Ok(lock)
    }

    /// Load an existing overlay or create a new one.
    ///
    /// Acquires the directory lock internally.  If you need to hold the lock
    /// across a wipe-and-retry cycle, use [`Self::acquire_lock`] +
    /// [`Self::load_with_lock`] instead.
    pub fn load(
        dir: PathBuf,
        env_hash: String,
        transport: String,
        on_mismatch: crate::OverlayMismatch,
    ) -> Result<Self, OverlayError> {
        let lock = Self::acquire_lock(&dir)?;
        Self::load_with_lock(dir, env_hash, transport, on_mismatch, lock)
    }

    /// Load an existing overlay using a pre-acquired lock.
    ///
    /// The `lock` must have been obtained via [`Self::acquire_lock`] on the same
    /// directory.  This variant exists so `create_overlay` can hold the lock
    /// across the wipe-and-retry path without a race window.
    pub fn load_with_lock(
        dir: PathBuf,
        env_hash: String,
        transport: String,
        on_mismatch: crate::OverlayMismatch,
        lock: fs::File,
    ) -> Result<Self, OverlayError> {
        fs::create_dir_all(&dir)?;
        let state_path = dir.join(STATE_FILENAME);

        let (whiteouts, opaque_dirs) = if state_path.exists() {
            let content = fs::read_to_string(&state_path)?;
            let state: StateFile = serde_json::from_str(&content)?;
            if state.version != STATE_VERSION {
                return Err(OverlayError::VersionMismatch {
                    expected: STATE_VERSION,
                    found: state.version,
                    lock,
                });
            }
            if state.env_hash != env_hash {
                match on_mismatch {
                    crate::OverlayMismatch::Error => {
                        return Err(OverlayError::EnvHashMismatch {
                            expected: env_hash,
                            found: state.env_hash,
                            lock,
                        });
                    }
                    crate::OverlayMismatch::Adopt => {
                        // A library-level diagnostic only — surfacing this to the
                        // user is the caller's responsibility (it knows the policy
                        // name and where its output goes). Logged at debug so it
                        // does not double up with the caller's own message.
                        tracing::debug!(
                            overlay = %dir.display(),
                            recorded_env_hash = %state.env_hash,
                            new_env_hash = %env_hash,
                            "adopting overlay created for a different environment; \
                             keeping its contents and updating the recorded hash",
                        );
                        // Fall through: keep the existing whiteouts/opaque dirs and
                        // persist the new env hash on the flush below.
                    }
                }
            }
            if !state.transport.is_empty() && state.transport != transport {
                return Err(OverlayError::TransportMismatch {
                    expected: transport,
                    found: state.transport,
                    lock,
                });
            }
            (
                state.whiteouts.into_iter().collect(),
                state.opaque_dirs.into_iter().collect(),
            )
        } else {
            (HashSet::new(), HashSet::new())
        };

        let overlay = OverlayState {
            dir,
            whiteouts,
            opaque_dirs,
            state_path,
            env_hash,
            transport,
            _lock: lock,
        };
        // Write initial state file if it didn't exist
        overlay.flush()?;
        Ok(overlay)
    }

    /// Check if a virtual path has been whiteout'd (deleted).
    pub fn is_whiteout(&self, path: &Path) -> bool {
        self.whiteouts.contains(path)
    }

    /// Mark a virtual path as deleted. Flushes state to disk.
    pub fn add_whiteout(&mut self, path: PathBuf) -> Result<(), OverlayError> {
        self.whiteouts.insert(path);
        self.flush()
    }

    /// Remove a whiteout (e.g. when recreating a deleted file). Flushes state to disk.
    pub fn remove_whiteout(&mut self, path: &Path) -> Result<(), OverlayError> {
        if self.whiteouts.remove(path) {
            self.flush()?;
        }
        Ok(())
    }

    /// Get the path in the upper layer for a given virtual path.
    pub fn upper_path(&self, virtual_path: &Path) -> PathBuf {
        self.dir.join(virtual_path)
    }

    /// Check if a file exists in the upper layer.
    pub fn has_upper(&self, virtual_path: &Path) -> bool {
        self.upper_path(virtual_path).exists()
    }

    /// Check if a directory is opaque (lower layer hidden entirely).
    pub fn is_opaque(&self, path: &Path) -> bool {
        self.opaque_dirs.contains(path)
    }

    /// Mark a directory as opaque. Flushes state to disk.
    pub fn add_opaque_dir(&mut self, path: PathBuf) -> Result<(), OverlayError> {
        self.opaque_dirs.insert(path);
        self.flush()
    }

    /// Remove opaque marker from a directory. Flushes state to disk.
    pub fn remove_opaque_dir(&mut self, path: &Path) -> Result<(), OverlayError> {
        self.opaque_dirs.remove(path);
        self.flush()
    }

    /// The overlay directory path.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Atomically write state to disk (write-tmp → fsync → rename).
    pub(crate) fn flush(&self) -> Result<(), OverlayError> {
        let state = StateFile {
            version: STATE_VERSION,
            env_hash: self.env_hash.clone(),
            transport: self.transport.clone(),
            whiteouts: self.whiteouts.iter().cloned().collect(),
            opaque_dirs: self.opaque_dirs.iter().cloned().collect(),
        };
        let json = serde_json::to_string_pretty(&state)?;

        let tmp_path = self.state_path.with_extension("tmp");
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp_path, &self.state_path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_load_creates_new_state() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("overlay");
        let state = OverlayState::load(
            dir.clone(),
            "hash123".into(),
            "test".into(),
            crate::OverlayMismatch::Error,
        )
        .unwrap();

        assert!(state.whiteouts.is_empty());
        assert!(dir.join(STATE_FILENAME).exists());

        let content = fs::read_to_string(dir.join(STATE_FILENAME)).unwrap();
        let parsed: StateFile = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.env_hash, "hash123");
        assert!(parsed.whiteouts.is_empty());
    }

    #[test]
    fn test_load_verifies_hash() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("overlay");

        // Create with one hash
        OverlayState::load(
            dir.clone(),
            "hash_a".into(),
            "test".into(),
            crate::OverlayMismatch::Error,
        )
        .unwrap();

        // Try to load with different hash
        let err = OverlayState::load(
            dir,
            "hash_b".into(),
            "test".into(),
            crate::OverlayMismatch::Error,
        )
        .unwrap_err();
        assert!(matches!(err, OverlayError::EnvHashMismatch { .. }));
    }

    #[test]
    fn test_load_adopts_on_mismatch() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("overlay");

        // Create with one hash and record a whiteout.
        {
            let mut state = OverlayState::load(
                dir.clone(),
                "hash_a".into(),
                "test".into(),
                crate::OverlayMismatch::Error,
            )
            .unwrap();
            state.add_whiteout(PathBuf::from("lib/foo.py")).unwrap();
        }

        // Loading with a different hash under the Adopt policy succeeds, keeps the
        // existing whiteout, and persists the new hash on the next flush.
        let mut state = OverlayState::load(
            dir.clone(),
            "hash_b".into(),
            "test".into(),
            crate::OverlayMismatch::Adopt,
        )
        .unwrap();
        assert!(state.is_whiteout(Path::new("lib/foo.py")));

        state.add_whiteout(PathBuf::from("lib/bar.py")).unwrap();
        let content = fs::read_to_string(dir.join(STATE_FILENAME)).unwrap();
        let parsed: StateFile = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.env_hash, "hash_b");
        assert_eq!(parsed.whiteouts.len(), 2);
    }

    #[test]
    fn test_load_accepts_matching_hash() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("overlay");

        OverlayState::load(
            dir.clone(),
            "hash_a".into(),
            "test".into(),
            crate::OverlayMismatch::Error,
        )
        .unwrap();
        let state = OverlayState::load(
            dir,
            "hash_a".into(),
            "test".into(),
            crate::OverlayMismatch::Error,
        )
        .unwrap();
        assert!(state.whiteouts.is_empty());
    }

    #[test]
    fn test_whiteout_add_remove() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("overlay");
        let mut state = OverlayState::load(
            dir,
            "hash".into(),
            "test".into(),
            crate::OverlayMismatch::Error,
        )
        .unwrap();

        let path = PathBuf::from("lib/foo.py");
        assert!(!state.is_whiteout(&path));

        state.add_whiteout(path.clone()).unwrap();
        assert!(state.is_whiteout(&path));

        state.remove_whiteout(&path).unwrap();
        assert!(!state.is_whiteout(&path));
    }

    #[test]
    fn test_whiteout_persists_across_reload() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("overlay");

        // Add a whiteout
        {
            let mut state = OverlayState::load(
                dir.clone(),
                "hash".into(),
                "test".into(),
                crate::OverlayMismatch::Error,
            )
            .unwrap();
            state.add_whiteout(PathBuf::from("lib/deleted.py")).unwrap();
        }

        // Reload and verify
        let state = OverlayState::load(
            dir,
            "hash".into(),
            "test".into(),
            crate::OverlayMismatch::Error,
        )
        .unwrap();
        assert!(state.is_whiteout(Path::new("lib/deleted.py")));
    }

    #[test]
    fn test_upper_path() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("overlay");
        let state = OverlayState::load(
            dir.clone(),
            "hash".into(),
            "test".into(),
            crate::OverlayMismatch::Error,
        )
        .unwrap();

        assert_eq!(
            state.upper_path(Path::new("lib/foo.py")),
            dir.join("lib/foo.py")
        );
    }

    #[test]
    fn test_has_upper() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("overlay");
        let state = OverlayState::load(
            dir.clone(),
            "hash".into(),
            "test".into(),
            crate::OverlayMismatch::Error,
        )
        .unwrap();

        assert!(!state.has_upper(Path::new("lib/foo.py")));

        // Create the file in upper
        fs::create_dir_all(dir.join("lib")).unwrap();
        fs::write(dir.join("lib/foo.py"), b"content").unwrap();

        assert!(state.has_upper(Path::new("lib/foo.py")));
    }

    #[test]
    fn test_flush_produces_valid_json() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("overlay");
        let mut state = OverlayState::load(
            dir.clone(),
            "hash".into(),
            "test".into(),
            crate::OverlayMismatch::Error,
        )
        .unwrap();

        state.add_whiteout(PathBuf::from("a/b.py")).unwrap();
        state.add_whiteout(PathBuf::from("c/d.py")).unwrap();

        // Read and verify the state file is valid JSON
        let content = fs::read_to_string(dir.join(STATE_FILENAME)).unwrap();
        let parsed: StateFile = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.env_hash, "hash");
        assert_eq!(parsed.whiteouts.len(), 2);
    }
}
