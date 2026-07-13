//! Virtual conda environment mounts.
//!
//! `rattler_vfs` presents a conda environment as a virtual filesystem, serving
//! files directly from the package cache with on-the-fly prefix replacement.
//! No files are copied to disk for read-only use; a persistent copy-on-write
//! overlay enables writes (e.g. `pip install`) without modifying the cache.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use rattler_vfs::{MountConfig, Transport, build_and_mount, compute_env_hash};
//! use rattler_cache::{default_cache_dir, package_cache::PackageCache};
//! use rattler_conda_types::Platform;
//! use rattler_lock::LockFile;
//! # async fn example() -> anyhow::Result<()> {
//! let lockfile = LockFile::from_path("pixi.lock".as_ref())?;
//! let platform = Platform::current();
//! let env_hash = compute_env_hash(&lockfile, "default", platform)?;
//! let cache = PackageCache::new(default_cache_dir()?.join("pkgs"));
//!
//! let config = MountConfig::new_read_only(
//!     "/path/to/env".into(),
//!     Transport::Auto,
//!     env_hash,
//! );
//! let handle = build_and_mount(&lockfile, "default", platform, &cache, &config).await?;
//! // Environment is live at /path/to/env.
//! // Dropping `handle` unmounts; call `handle.unmount().await` for explicit error handling.
//! # Ok(())
//! # }
//! ```
//!
//! # Platform support
//!
//! | Platform | Default backend | Available |
//! |----------|-----------------|-----------|
//! | Linux | FUSE | FUSE, NFS |
//! | macOS | NFS | NFS, FUSE (requires [macFUSE]) |
//! | Windows | [ProjFS] | `ProjFS` |
//!
//! [`Transport::Auto`] selects the default for the current platform.
//!
//! **Why NFS on macOS?** FUSE on macOS requires [macFUSE], a third-party
//! kernel extension that needs System Integrity Protection (SIP) to be
//! reduced on Apple Silicon. Additionally, FUSE mounts lose all kernel vnode
//! code-signature caches on unmount, causing a significant Gatekeeper
//! re-verification penalty on every remount. The NFS transport uses macOS's
//! built-in NFS client, avoiding both issues.
//!
//! **Why FUSE on Linux?** FUSE has lower overhead than NFS on Linux (no TCP
//! stack, no marshalling) and supports kernel-level page caching and passthrough
//! I/O. The NFS backend is available as a fallback.
//!
//! **Why `ProjFS` on Windows?** [ProjFS] is built into Windows 10+ and mounts to
//! any directory without elevation. Its demand-driven callback model
//! ("materialize files when accessed") maps naturally onto virtual environments.
//! NFS is not supported as a transport on Windows due to client limitations
//! (portmapper requirements, drive-letter-only mounts, `NFSv2` fallback).
//!
//! **macOS alternatives under investigation:** [FSKit] is Apple's modern
//! successor to kernel extensions for filesystems, but current known
//! implementations target block-style storage rather than projected/virtual
//! filesystems.
//!
//! [macFUSE]: https://osxfuse.github.io/
//! [ProjFS]: https://learn.microsoft.com/en-us/windows/win32/projfs/projected-file-system
//! [FSKit]: https://developer.apple.com/documentation/fskit

#[cfg(target_os = "macos")]
pub mod codesign;
#[cfg(any(target_os = "linux", feature = "fuse"))]
pub mod fuse_adapter;
pub(crate) mod metadata_tree;
#[cfg(feature = "nfs")]
pub mod nfs_adapter;
pub mod overlay;
pub mod overlay_fs;
pub mod prefix_replacement;
#[cfg(target_os = "windows")]
pub mod projfs_adapter;
pub mod vfs_ops;
pub mod virtual_fs;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use metadata_tree::MetadataNode;
use rattler::install::PythonInfo;
use rattler::install::python_entry_point_template;
use rattler_cache::package_cache::PackageCache;
use rattler_conda_types::Platform;
use rattler_conda_types::package::{EntryPoint, LinkJson, NoArchLinks, PackageFile, PathsJson};
use rattler_lock::LockFile;
use rattler_networking::LazyClient;
use virtual_fs::VirtualFS;

// ---------------------------------------------------------------------------
// Structured errors for downstream consumers (pixi)
// ---------------------------------------------------------------------------

/// Errors from `rattler_vfs` that downstream consumers can match on.
///
/// Most `rattler_vfs` functions return `anyhow::Result` for convenience, with
/// these variants as the underlying cause when a structured match is needed.
/// Use `anyhow::Error::downcast_ref::<MountError>()` to extract them.
#[derive(Debug, thiserror::Error)]
pub enum MountError {
    /// The requested environment was not found in the lock file.
    #[error("environment '{name}' not found in lock file")]
    EnvironmentNotFound { name: String },

    /// No packages for the requested platform in the environment.
    #[error("no packages for platform {platform} in environment '{environment}'")]
    PlatformNotFound {
        platform: Platform,
        environment: String,
    },

    /// The `ProjFS` optional Windows feature is not enabled.
    #[error(
        "Windows Projected File System (ProjFS) is not available.\n\
         Enable it with (requires Administrator):\n\n  \
         Enable-WindowsOptionalFeature -Online -FeatureName Client-ProjFS -NoRestart\n"
    )]
    ProjFsDllMissing,

    /// `ProjFS` does not support read-only mode.
    #[error(
        "ProjFS does not support read-only mode: it lacks a pre-creation \
         notification, so new files can always be created. Use Mode::Writable \
         instead (overlay_dir is ignored on ProjFS)."
    )]
    ProjFsReadOnlyUnsupported,

    /// Linux NFS mount requires passwordless sudo.
    #[error(
        "Linux NFS mount requires passwordless sudo (the NFS client needs \
         CAP_SYS_ADMIN). Configure passwordless sudo for `mount -t nfs` or \
         use Transport::Fuse instead."
    )]
    SudoRequired,

    /// The overlay's environment hash doesn't match the current lock file.
    #[error(
        "the overlay at {} was created for a different environment (expected \
         hash '{expected}', found '{found}'). The overlay may contain files \
         you want to keep.\n\
         To reset the overlay for the new environment, remove it and remount:\n  \
         rm -rf {}",
        .overlay_dir.display(),
        .overlay_dir.display()
    )]
    OverlayEnvHashMismatch {
        expected: String,
        found: String,
        overlay_dir: PathBuf,
    },

    /// The overlay directory was created by a different transport.
    #[error(
        "overlay was created with transport '{found}' but the current mount \
         requested '{expected}'. Remove the overlay manually or switch back \
         to the original transport."
    )]
    OverlayTransportMismatch { expected: String, found: String },

    /// The requested transport is not available on this platform.
    #[error("transport {transport:?} not available (missing feature or unsupported platform)")]
    TransportNotAvailable { transport: Transport },
}

/// Build a virtual directory tree from a package's `PathsJson`.
///
/// Each call extends `env_paths` and `directory_indices` with the entries
/// from one package. The `cache_path` should point to the extracted package
/// directory in the cache.
///
/// For noarch Python packages, pass `python_info` to rewrite paths:
/// `site-packages/` → `lib/pythonX.Y/site-packages/` and
/// `python-scripts/` → `bin/`.
pub(crate) fn path_parse(
    paths_json: &PathsJson,
    cache_path: &Path,
    python_info: Option<&PythonInfo>,
    env_paths: &mut Vec<MetadataNode>,
    directory_indices: &mut HashMap<PathBuf, usize>,
) {
    let cachepath: Arc<Path> = cache_path.into();

    for path in &paths_json.paths {
        // For noarch Python packages, rewrite site-packages/ and python-scripts/ paths
        let (virtual_path, cache_prefix_override) = match python_info {
            Some(info) => {
                let rewritten = info.get_python_noarch_target_path(&path.relative_path);
                if rewritten.as_ref() == path.relative_path {
                    (path.relative_path.clone(), None)
                } else {
                    let original_parent = path
                        .relative_path
                        .parent()
                        .map_or_else(|| PathBuf::from("."), |p| PathBuf::from(".").join(p));
                    (rewritten.into_owned(), Some(original_parent))
                }
            }
            None => (path.relative_path.clone(), None),
        };

        let parent_directory = virtual_path.parent().unwrap_or(Path::new("."));
        let mut parent_index = 0;

        for component in parent_directory.components() {
            let current_path = env_paths[parent_index]
                .as_directory()
                .expect("parent is always a directory")
                .prefix_path
                .join(component);

            if let Some(&index) = directory_indices.get(&current_path) {
                parent_index = index;
            } else {
                let new_dir = MetadataNode::new_directory(current_path.clone(), parent_index);
                let child_index = env_paths.len();

                env_paths.push(new_dir);
                env_paths[parent_index]
                    .as_directory_mut()
                    .expect("parent is a directory")
                    .children
                    .push(child_index);

                directory_indices.insert(current_path, child_index);
                parent_index = child_index;
            }
        }

        let file_name = virtual_path.file_name().expect("files always have names");

        let file_index = env_paths.len();
        let mut file_entry = MetadataNode::new_file(
            file_name.into(),
            parent_index,
            cachepath.clone(),
            path.path_type,
            path.prefix_placeholder.clone(),
        );
        if let Some(ref override_path) = cache_prefix_override {
            file_entry.as_file_mut().unwrap().cache_prefix_path = Some(override_path.clone());
        }
        env_paths.push(file_entry);

        env_paths[parent_index]
            .as_directory_mut()
            .expect("parent is a directory")
            .children
            .push(file_index);
    }
}

/// Ensure a directory exists in the metadata tree, creating it if necessary.
/// Returns the index of the directory.
fn ensure_directory(
    dir_path: PathBuf,
    parent_index: usize,
    env_paths: &mut Vec<MetadataNode>,
    directory_indices: &mut HashMap<PathBuf, usize>,
) -> usize {
    if let Some(&index) = directory_indices.get(&dir_path) {
        return index;
    }
    let new_dir = MetadataNode::new_directory(dir_path.clone(), parent_index);
    let child_index = env_paths.len();
    env_paths.push(new_dir);
    env_paths[parent_index]
        .as_directory_mut()
        .expect("parent is a directory")
        .children
        .push(child_index);
    directory_indices.insert(dir_path, child_index);
    child_index
}

/// Generate noarch python entry point scripts and add them as virtual files
/// in the metadata tree.
pub(crate) fn add_entry_points(
    entry_points: &[EntryPoint],
    target_prefix: &str,
    python_info: &PythonInfo,
    env_paths: &mut Vec<MetadataNode>,
    directory_indices: &mut HashMap<PathBuf, usize>,
) {
    let bin_dir = PathBuf::from("./bin");
    let bin_index = ensure_directory(bin_dir, 0, env_paths, directory_indices);

    for ep in entry_points {
        let content = python_entry_point_template(target_prefix, false, ep, python_info);
        let file_index = env_paths.len();
        env_paths.push(MetadataNode::new_virtual_file(
            ep.command.as_str().into(),
            bin_index,
            content.into_bytes(),
        ));
        env_paths[bin_index]
            .as_directory_mut()
            .expect("bin is a directory")
            .children
            .push(file_index);
    }
}

/// Initialise the root directory and index for a new virtual filesystem tree.
pub(crate) fn new_empty_tree() -> (Vec<MetadataNode>, HashMap<PathBuf, usize>) {
    let env_paths = vec![MetadataNode::new_directory(PathBuf::from("."), 0)];
    let mut directory_indices = HashMap::new();
    directory_indices.insert(PathBuf::from("."), 0);
    (env_paths, directory_indices)
}

// ---------------------------------------------------------------------------
// Library API: mount orchestration
// ---------------------------------------------------------------------------

/// Opaque metadata tree produced by [`build_metadata_tree`].
///
/// Pass to [`mount`] or [`build_and_mount`]; the internal representation is
/// not stable and is intentionally not exposed. The newtype wrapper means
/// downstream consumers cannot construct one directly — guaranteeing every
/// mount went through `build_metadata_tree`'s validation.
pub struct MetadataTree(pub(crate) Vec<MetadataNode>);

/// Transport backend for the virtual filesystem.
///
/// See the [crate-level docs](crate#platform-support) for why each platform
/// has a different default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// Auto-detect the best backend for the current platform: FUSE on Linux,
    /// NFS on macOS, `ProjFS` on Windows.
    Auto,
    /// `NFSv3` userspace server on localhost. Works on all platforms without
    /// kernel extensions. On Windows, constrained to port 2049 and drive letters.
    ///
    /// **Linux note:** `mount -t nfs` requires `CAP_SYS_ADMIN`, which is not
    /// granted in unprivileged user namespaces. The adapter probes for
    /// passwordless `sudo` before attempting the mount and fails fast if it's
    /// not available. On Linux, prefer [`Transport::Fuse`] unless you
    /// specifically need NFS parity with macOS — [`Transport::Auto`] already
    /// picks FUSE.
    Nfs,
    /// FUSE via libfuse3 (Linux) or macFUSE (macOS, requires `fuse` feature).
    /// Not available on Windows.
    Fuse,
    /// Windows Projected File System. Demand-driven: files are materialized on
    /// first access. Only available on Windows 10 version 1809+.
    ProjFs,
}

impl Transport {
    /// Short name for state file tracking.
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Nfs => "nfs",
            Self::Fuse => "fuse",
            Self::ProjFs => "projfs",
        }
    }

    /// Resolve `Auto` to the platform-appropriate transport.
    pub fn resolve(self) -> Self {
        match self {
            Self::Auto => {
                if cfg!(target_os = "windows") {
                    Self::ProjFs
                } else if cfg!(target_os = "macos") {
                    Self::Nfs
                } else {
                    Self::Fuse
                }
            }
            other => other,
        }
    }

    /// Whether this transport is available on the current platform and build.
    ///
    /// Use this at config-parse time to reject invalid combinations early
    /// instead of waiting for [`mount`] to fail at runtime.
    pub fn is_available(self) -> bool {
        match self.resolve() {
            Self::Auto => unreachable!("resolve() never returns Auto"),
            Self::Fuse => cfg!(any(target_os = "linux", feature = "fuse")),
            Self::Nfs => cfg!(feature = "nfs"),
            Self::ProjFs => cfg!(target_os = "windows"),
        }
    }
}

/// Whether the mount is read-only or writable, and where the writable
/// overlay lives.
///
/// `ProjFS` does not support read-only mode (it lacks a pre-creation
/// notification, so new files can always be created via the virtualization
/// root) — passing [`Mode::ReadOnly`] to a `ProjFS` mount returns a clear error.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Mode {
    /// Read-only mount. Writes return `EROFS`. Not supported on `ProjFS` —
    /// use [`Mode::ReadOnlyIfSupported`] for cross-platform configs.
    ReadOnly,

    /// Read-only if the transport supports it, otherwise writable.
    ///
    /// On FUSE/NFS this behaves identically to [`Mode::ReadOnly`].
    /// On `ProjFS` (which cannot enforce read-only) this silently falls
    /// through to writable mode and logs a warning. Use this in pixi
    /// configs where `mount-read-only = true` should work cross-platform.
    ReadOnlyIfSupported,

    /// Writable mount. Writes go to a persistent copy-on-write overlay.
    ///
    /// For FUSE/NFS, `overlay_dir` is a separate persistent directory pinned
    /// to a specific environment via [`MountConfig::env_hash`].
    ///
    /// For `ProjFS`, `overlay_dir` is ignored — `ProjFS` writes hydrated
    /// content directly to the virtualization root (the mount point) and
    /// tracks deletions via tombstones. Pass `overlay_dir: None` for `ProjFS`.
    Writable {
        /// Persistent overlay directory for FUSE/NFS. `None` is valid only
        /// for `ProjFS`, which uses the mount point itself.
        overlay_dir: Option<PathBuf>,
    },
}

/// What to do when the persistent overlay was created for a different version
/// of the environment (its recorded env hash no longer matches the one being
/// mounted, e.g. after `pixi add`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayMismatch {
    /// Refuse to mount and return [`MountError::OverlayEnvHashMismatch`].
    #[default]
    Error,
    /// Reuse the existing overlay on top of the new environment, updating its
    /// recorded hash. Preserves overlay writes (e.g. `pip install` results)
    /// across environment changes, at the small risk of a stale entry if the
    /// change also modified a file that had been copied up.
    Adopt,
}

/// Configuration for mounting a virtual environment.
///
/// Marked `#[non_exhaustive]` so new fields can be added without a `SemVer`
/// break. Construct via [`MountConfig::new_read_only`] or
/// [`MountConfig::new_writable`], optionally chaining
/// [`with_allow_other`](MountConfig::with_allow_other) or
/// [`with_overlay_mismatch`](MountConfig::with_overlay_mismatch).
#[non_exhaustive]
pub struct MountConfig {
    /// Directory where the virtual environment will appear.
    pub mount_point: PathBuf,

    /// Read-only or writable, and where the overlay lives.
    pub mode: Mode,

    /// Transport backend. Use [`Transport::Auto`] to let the platform decide.
    pub transport: Transport,

    /// Identity hash of the resolved environment, used to detect when the
    /// environment has changed and the overlay needs to be reset. Compute
    /// with [`compute_env_hash`].
    pub env_hash: String,

    /// Allow other users to access the mount. Only applies to FUSE; requires
    /// `user_allow_other` in `/etc/fuse.conf`. Most use cases don't need this.
    pub allow_other: bool,

    /// What to do when the persistent overlay was created for a different
    /// version of the environment. Defaults to [`OverlayMismatch::Error`].
    pub overlay_mismatch: OverlayMismatch,
}

impl MountConfig {
    /// Read-only mount. Writes return `EROFS`.
    pub fn new_read_only(mount_point: PathBuf, transport: Transport, env_hash: String) -> Self {
        Self {
            mount_point,
            mode: Mode::ReadOnly,
            transport,
            env_hash,
            allow_other: false,
            overlay_mismatch: OverlayMismatch::Error,
        }
    }

    /// Read-only if the transport supports it, otherwise writable.
    ///
    /// On FUSE/NFS this behaves like [`Self::new_read_only`]. On `ProjFS` —
    /// which cannot enforce read-only, since it has no pre-creation
    /// notification — it falls through to a writable mount (logging a warning)
    /// instead of erroring with [`MountError::ProjFsReadOnlyUnsupported`]. Use
    /// this for cross-platform configs where `mount-read-only = true` should
    /// still work on Windows.
    pub fn new_read_only_if_supported(
        mount_point: PathBuf,
        transport: Transport,
        env_hash: String,
    ) -> Self {
        Self {
            mount_point,
            mode: Mode::ReadOnlyIfSupported,
            transport,
            env_hash,
            allow_other: false,
            overlay_mismatch: OverlayMismatch::Error,
        }
    }

    /// Writable mount with a persistent COW overlay.
    ///
    /// `overlay_dir` is required for FUSE/NFS and must be a separate directory
    /// from `mount_point`. Pass `None` for `ProjFS` — `ProjFS` uses the mount
    /// point itself as the virtualization root.
    pub fn new_writable(
        mount_point: PathBuf,
        overlay_dir: Option<PathBuf>,
        transport: Transport,
        env_hash: String,
    ) -> Self {
        Self {
            mount_point,
            mode: Mode::Writable { overlay_dir },
            transport,
            env_hash,
            allow_other: false,
            overlay_mismatch: OverlayMismatch::Error,
        }
    }

    /// Allow other users to access the mount (FUSE only).
    pub fn with_allow_other(mut self, allow_other: bool) -> Self {
        self.allow_other = allow_other;
        self
    }

    /// Set what to do when the overlay was created for a different environment.
    pub fn with_overlay_mismatch(mut self, overlay_mismatch: OverlayMismatch) -> Self {
        self.overlay_mismatch = overlay_mismatch;
        self
    }
}

/// Handle to a running mount.
///
/// The mount stays live for as long as this handle exists. Dropping it
/// triggers a best-effort unmount and stops the background server (NFS) or
/// session (FUSE). Use [`MountHandle::unmount`] for explicit, error-returning
/// unmount; prefer it over relying on Drop when error handling matters.
///
/// Marked `#[non_exhaustive]` so new transport variants can be added without
/// a `SemVer` break. Downstream `match` arms must include `_ =>` to be exhaustive.
#[non_exhaustive]
pub enum MountHandle {
    #[cfg(feature = "nfs")]
    Nfs(nfs_adapter::NfsMountHandle),
    #[cfg(any(target_os = "linux", feature = "fuse"))]
    Fuse(fuser::BackgroundSession),
    #[cfg(target_os = "windows")]
    ProjFs(projfs_adapter::ProjFsHandle),
}

impl MountHandle {
    /// Whether the mount's backing server is still running.
    ///
    /// Currently only meaningful for the NFS transport, where the userspace
    /// server task can exit unexpectedly (panic, I/O error, or unexpected
    /// clean return) leaving the kernel mount stale. FUSE and `ProjFS` mounts
    /// are managed by the kernel and always report healthy from userspace.
    ///
    /// Pixi's `MountGuard` can poll this to detect a dead server before
    /// handing out a reference to the environment.
    #[allow(unreachable_patterns)]
    pub fn is_healthy(&self) -> bool {
        match self {
            #[cfg(feature = "nfs")]
            Self::Nfs(h) => h.is_healthy(),
            _ => true,
        }
    }

    /// Explicitly unmount the filesystem and shut down the backing server.
    ///
    /// This is async so the NFS unmount path can use `tokio::process::Command`
    /// instead of blocking the runtime. FUSE and `ProjFS` unmount synchronously
    /// (kernel-managed, no subprocess) so the async boundary is free for them.
    ///
    /// Prefer this over relying on Drop when error handling matters (e.g.
    /// sidecar shutdown, CI cleanup, signal-handling paths). Drop stays as a
    /// best-effort fallback that logs failures but cannot return them.
    #[allow(unreachable_patterns)]
    pub async fn unmount(self) -> anyhow::Result<()> {
        match self {
            #[cfg(feature = "nfs")]
            Self::Nfs(h) => h.unmount().await,
            #[cfg(any(target_os = "linux", feature = "fuse"))]
            Self::Fuse(session) => {
                // fuser does not expose a Result-returning unmount; dropping
                // the BackgroundSession is the documented shutdown path.
                drop(session);
                Ok(())
            }
            #[cfg(target_os = "windows")]
            Self::ProjFs(h) => h.unmount(),
            _ => Ok(()),
        }
    }
}

/// Build the in-memory metadata tree from a parsed lock file.
///
/// Fetches packages from `package_cache` as needed, reads `PathsJson` for each,
/// and constructs the virtual directory tree with noarch Python path rewriting
/// and entry point generation.
///
/// Caller responsibilities:
/// - Parse the lock file once via [`LockFile::from_path`].
/// - Pick a [`Platform`] (usually [`Platform::current()`]).
/// - Construct a [`PackageCache`] (commonly via
///   [`rattler_cache::default_cache_dir()`]). Decoupling the cache from this
///   function lets pixi share its own cache and lets tests use a temp dir.
pub async fn build_metadata_tree(
    lockfile: &LockFile,
    environment_name: &str,
    platform: Platform,
    package_cache: &PackageCache,
    mount_point: &Path,
) -> anyhow::Result<MetadataTree> {
    let environment =
        lockfile
            .environment(environment_name)
            .ok_or(MountError::EnvironmentNotFound {
                name: environment_name.to_string(),
            })?;
    let package_refs: Vec<_> = lockfile
        .platform(platform.as_str())
        .and_then(|p| environment.packages(p))
        .ok_or(MountError::PlatformNotFound {
            platform,
            environment: environment_name.to_string(),
        })?
        .collect();

    let python_info = package_refs
        .iter()
        .filter_map(|p| p.as_binary_conda())
        .find(|p| p.package_record.name.as_normalized() == "python")
        .map(|p| PythonInfo::from_python_record(&p.package_record, platform))
        .transpose()
        .map_err(|e| anyhow::anyhow!("failed to get python info: {e}"))?;

    let (mut env_paths, mut directory_indices) = new_empty_tree();
    let mount_str = mount_point.to_string_lossy().to_string();

    // Build a single lazily-initialized HTTP client for the whole package loop.
    // `LazyClient::default()` forces construction eagerly, which on macOS walks
    // the keychain via `rustls_native_certs` and takes several seconds. Using
    // `LazyClient::new` defers that work until the first cache miss, so
    // warm-cache mounts skip it entirely.
    let client = LazyClient::new(reqwest_middleware::ClientWithMiddleware::default);

    // Validate all packages up front so we can parallelize fetching.
    let mut conda_packages: Vec<_> = package_refs
        .iter()
        .filter_map(|p| p.as_binary_conda())
        .collect();

    // Sort largest first so long downloads start early (mirrors rattler's
    // installer pattern at installer/mod.rs:600).
    conda_packages.sort_by(|a, b| {
        b.package_record
            .size
            .unwrap_or(0)
            .cmp(&a.package_record.size.unwrap_or(0))
    });

    // Fetch + parse packages in parallel. The tree mutation (path_parse)
    // stays serial because env_paths/directory_indices are shared mutable
    // state — the downloads and JSON parses are the expensive parts.
    let concurrency = Arc::new(tokio::sync::Semaphore::new(16));
    let mut join_set = tokio::task::JoinSet::new();

    for package_data in &conda_packages {
        let cache = package_cache.clone();
        let client = client.clone();
        let record = package_data.package_record.clone();
        let location = package_data.location.clone();
        let is_noarch_python = package_data.package_record.noarch.is_python();
        let sem = concurrency.clone();

        join_set.spawn(async move {
            let _permit = sem
                .acquire()
                .await
                .map_err(|e| anyhow::anyhow!("concurrency semaphore closed: {e}"))?;

            let url = location
                .as_url()
                .ok_or_else(|| anyhow::anyhow!("package has no URL"))?
                .clone();
            let cache_metadata = cache
                .get_or_fetch_from_url_with_retry(
                    &record,
                    url,
                    client,
                    rattler_networking::retry_policies::default_retry_policy(),
                    None,
                    // rattler_vfs limits concurrency via its own semaphore
                    // (acquired above), so don't apply the cache's limiter too.
                    None,
                )
                .await?;

            // Parse paths.json inside the spawned task to avoid blocking the
            // main runtime thread.
            let path = cache_metadata.path().to_path_buf();
            let paths_json = PathsJson::from_package_directory_with_deprecated_fallback(&path)?;

            // For noarch python packages, also load link.json for entry points.
            let entry_points: Vec<EntryPoint> = if is_noarch_python {
                LinkJson::from_package_directory(&path)
                    .ok()
                    .and_then(|lj| match lj.noarch {
                        NoArchLinks::Python(ep) => Some(ep.entry_points),
                        NoArchLinks::Generic => None,
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };

            Ok::<_, anyhow::Error>((path, paths_json, is_noarch_python, entry_points))
        });
    }

    // Collect results and build the tree serially.
    while let Some(result) = join_set.join_next().await {
        let (cache_path, paths_json, is_noarch_python, entry_points) =
            result.map_err(|e| anyhow::anyhow!("fetch task failed: {e}"))??;

        let noarch_python_info = if is_noarch_python {
            python_info.as_ref()
        } else {
            None
        };
        path_parse(
            &paths_json,
            &cache_path,
            noarch_python_info,
            &mut env_paths,
            &mut directory_indices,
        );

        if let Some(ref python_info) = python_info
            && !entry_points.is_empty()
        {
            add_entry_points(
                &entry_points,
                &mount_str,
                python_info,
                &mut env_paths,
                &mut directory_indices,
            );
        }

        tracing::debug!("parsed {} metadata entries", env_paths.len());
    }

    Ok(MetadataTree(env_paths))
}

/// Mount a pre-built metadata tree. Returns a handle that unmounts on drop.
pub async fn mount(metadata: MetadataTree, config: &MountConfig) -> anyhow::Result<MountHandle> {
    let transport = config.transport.resolve();

    // ProjFS-specific pre-flight checks: DLL availability, mode validity,
    // overlay state. Done before VFS construction so we don't waste offset
    // computation if ProjFS isn't installed or the user passed Mode::ReadOnly.
    #[cfg(target_os = "windows")]
    if matches!(transport, Transport::ProjFs) {
        // Verify that the ProjFS optional feature is enabled before calling
        // any ProjFS API.  The `windows` crate delay-loads the DLL, so a
        // missing feature won't crash the process, but the first API call
        // would return a confusing "not found" HRESULT.  Give users a clear
        // message instead.
        {
            use std::os::windows::ffi::OsStrExt;
            let dll: Vec<u16> = std::ffi::OsStr::new("projectedfslib.dll")
                .encode_wide()
                .chain(Some(0))
                .collect();
            let handle = unsafe {
                windows::Win32::System::LibraryLoader::LoadLibraryW(windows::core::PCWSTR(
                    dll.as_ptr(),
                ))
            };
            if handle.is_err() {
                return Err(MountError::ProjFsDllMissing.into());
            }
        }

        // ProjFS is always writable — it writes hydrated content directly
        // to the virtualization root and tracks deletions via tombstones.
        // There is no read-only mode: ProjFS lacks a pre-creation
        // notification, so new files can always be created.
        if matches!(config.mode, Mode::ReadOnly) {
            return Err(MountError::ProjFsReadOnlyUnsupported.into());
        }
        if matches!(config.mode, Mode::ReadOnlyIfSupported) {
            tracing::warn!("ProjFS does not support read-only mode; falling through to writable");
        }

        // Validate overlay state (env hash) to reject stale mounts.
        {
            use crate::overlay::{OverlayError, OverlayState};
            match OverlayState::load(
                config.mount_point.clone(),
                config.env_hash.clone(),
                "projfs".to_string(),
                config.overlay_mismatch,
            ) {
                Ok(_) => {} // hash matches or fresh overlay
                Err(OverlayError::EnvHashMismatch {
                    expected, found, ..
                }) => {
                    return Err(MountError::OverlayEnvHashMismatch {
                        expected,
                        found,
                        // For ProjFS the overlay is the virtualization root,
                        // i.e. the mount point itself.
                        overlay_dir: config.mount_point.clone(),
                    }
                    .into());
                }
                Err(OverlayError::TransportMismatch {
                    expected, found, ..
                }) => {
                    return Err(MountError::OverlayTransportMismatch { expected, found }.into());
                }
                Err(e) => anyhow::bail!("overlay state check failed: {e}"),
            }
        }
    }

    // Construct the VirtualFS once. Each transport branch consumes it.
    // VFS construction does eager prefix-offset computation, so we want
    // exactly one call per mount.
    let vfs = VirtualFS::new(metadata.0, &config.mount_point);

    match transport {
        #[cfg(feature = "nfs")]
        Transport::Nfs => Ok(MountHandle::Nfs(mount_nfs(vfs, config).await?)),
        #[cfg(any(target_os = "linux", feature = "fuse"))]
        Transport::Fuse => Ok(MountHandle::Fuse(mount_fuse(vfs, config)?)),
        #[cfg(target_os = "windows")]
        Transport::ProjFs => {
            let adapter = projfs_adapter::ProjFsAdapter::new(vfs);
            let handle = adapter.start(&config.mount_point)?;
            Ok(MountHandle::ProjFs(handle))
        }
        #[allow(unreachable_patterns)]
        _ => Err(MountError::TransportNotAvailable { transport })?,
    }
}

/// Build the metadata tree from a lock file and mount it.
///
/// This is the main entry point for library consumers. It looks up the
/// environment + platform in `lockfile`, fetches each package via
/// `package_cache`, constructs the virtual directory tree, and mounts it.
/// Returns a [`MountHandle`] that unmounts on drop (or call
/// [`MountHandle::unmount`] for explicit error handling).
pub async fn build_and_mount(
    lockfile: &LockFile,
    environment_name: &str,
    platform: Platform,
    package_cache: &PackageCache,
    config: &MountConfig,
) -> anyhow::Result<MountHandle> {
    let metadata = build_metadata_tree(
        lockfile,
        environment_name,
        platform,
        package_cache,
        &config.mount_point,
    )
    .await?;
    mount(metadata, config).await
}

/// Schema version for [`compute_env_hash`].  Bump when the canonical form
/// changes so overlays are intentionally invalidated rather than silently
/// drifting.  The golden test `test_env_hash_stability` will fail when this
/// is bumped, reminding the author to update the expected hash.
pub const ENV_HASH_SCHEMA_VERSION: u32 = 2;

/// Compute an environment identity hash scoped to a single `(env, platform)`.
///
/// Hashes only the resolved package list for `environment_name` on `platform`,
/// **not** the entire lock-file bytes. Two consequences:
///
/// 1. Editing environment B does not invalidate overlays for environment A.
///    The pixi sidecar can keep one overlay per `(lockfile, env, platform)`
///    tuple without thrashing on unrelated changes.
/// 2. Reformatting the lock file or reordering its packages does not change
///    the hash, because each package is built into an explicit canonical
///    string (not serde-derived) and the strings are sorted before hashing.
///
/// The canonical form uses `\0`-separated fields per package to prevent
/// concatenation collisions.  Only fields that identify the package content
/// are included (name, version, build/subdir for conda, url for pypi, and
/// the content sha256 when available).
pub fn compute_env_hash(
    lockfile: &LockFile,
    environment_name: &str,
    platform: Platform,
) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};

    let environment =
        lockfile
            .environment(environment_name)
            .ok_or(MountError::EnvironmentNotFound {
                name: environment_name.to_string(),
            })?;
    let packages = lockfile
        .platform(platform.as_str())
        .and_then(|p| environment.packages(p))
        .ok_or(MountError::PlatformNotFound {
            platform,
            environment: environment_name.to_string(),
        })?;

    let mut hasher = Sha256::new();
    hasher.update(ENV_HASH_SCHEMA_VERSION.to_le_bytes());

    // Build an explicit canonical string per package using only the fields
    // that identify its content.  Sorted before hashing so reordering inside
    // the lockfile does not change the output.
    let mut package_keys: Vec<String> = packages
        .map(|pkg| match pkg {
            rattler_lock::LockedPackage::Conda(c) => {
                let record = c.record();
                let sha_hex = record
                    .and_then(|r| r.sha256.as_ref())
                    .map(hex::encode)
                    .unwrap_or_default();
                if sha_hex.is_empty() {
                    tracing::warn!(
                        "package {} has no sha256; env hash may collide across rebuilds",
                        c.name().as_normalized(),
                    );
                }
                format!(
                    "conda\0{}\0{}\0{}\0{}\0{}",
                    c.name().as_normalized(),
                    record.map(|r| r.version.to_string()).unwrap_or_default(),
                    record.map(|r| r.build.as_str()).unwrap_or_default(),
                    record.map(|r| r.subdir.as_str()).unwrap_or_default(),
                    sha_hex,
                )
            }
            rattler_lock::LockedPackage::Pypi(p) => {
                let sha_hex = p
                    .as_wheel()
                    .and_then(|w| w.hash.as_ref())
                    .and_then(|h| h.sha256())
                    .map(hex::encode)
                    .unwrap_or_default();
                format!(
                    "pypi\0{}\0{}\0{}\0{}",
                    p.name(),
                    p.version_string(),
                    p.location(),
                    sha_hex,
                )
            }
        })
        .collect();
    package_keys.sort();

    for key in &package_keys {
        hasher.update(key.as_bytes());
        hasher.update(b"\n");
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

// ---------------------------------------------------------------------------
// Internal: transport-specific mount helpers
// ---------------------------------------------------------------------------

/// Mount via FUSE, with optional writable overlay.
///
/// The overlay is retried once if the env hash mismatches (environment updated).
#[cfg(any(target_os = "linux", feature = "fuse"))]
fn mount_fuse(vfs: VirtualFS, config: &MountConfig) -> anyhow::Result<fuser::BackgroundSession> {
    use fuse_adapter::FuseAdapter;
    use fuser::{Config as FuserConfig, MountOption, SessionACL};

    let mut fuser_config = FuserConfig::default();
    fuser_config.mount_options = vec![
        MountOption::FSName("conda-packages".to_string()),
        MountOption::NoAtime,
    ];
    if matches!(config.mode, Mode::ReadOnly | Mode::ReadOnlyIfSupported) {
        fuser_config.mount_options.push(MountOption::RO);
    }
    if config.allow_other {
        fuser_config.acl = SessionACL::All;
    }

    match &config.mode {
        Mode::Writable {
            overlay_dir: Some(overlay_dir),
        } => {
            let overlay = create_overlay(
                vfs,
                overlay_dir,
                &config.env_hash,
                "fuse",
                config.overlay_mismatch,
            )?;
            let adapter = FuseAdapter::new(overlay);
            Ok(fuser::spawn_mount2(
                adapter,
                &config.mount_point,
                &fuser_config,
            )?)
        }
        Mode::Writable { overlay_dir: None } => {
            anyhow::bail!(
                "FUSE writable mode requires an overlay directory. Use \
                 MountConfig::new_writable(.., Some(overlay_dir), ..) or \
                 MountConfig::new_read_only(..) for a read-only mount."
            );
        }
        Mode::ReadOnly | Mode::ReadOnlyIfSupported => {
            let adapter = FuseAdapter::new(vfs);
            Ok(fuser::spawn_mount2(
                adapter,
                &config.mount_point,
                &fuser_config,
            )?)
        }
    }
}

/// Mount via NFS, with optional writable overlay.
///
/// The overlay is retried once if the env hash mismatches (environment updated).
#[cfg(feature = "nfs")]
async fn mount_nfs(
    vfs: VirtualFS,
    config: &MountConfig,
) -> anyhow::Result<nfs_adapter::NfsMountHandle> {
    use nfs_adapter::NfsAdapter;

    let read_only = matches!(config.mode, Mode::ReadOnly | Mode::ReadOnlyIfSupported);

    let bind_port = 0u16;

    let server_handle = match &config.mode {
        Mode::Writable {
            overlay_dir: Some(overlay_dir),
        } => {
            let overlay = create_overlay(
                vfs,
                overlay_dir,
                &config.env_hash,
                "nfs",
                config.overlay_mismatch,
            )?;
            NfsAdapter::new(overlay).serve(bind_port).await?
        }
        Mode::Writable { overlay_dir: None } => {
            anyhow::bail!(
                "NFS writable mode requires an overlay directory. Use \
                 MountConfig::new_writable(.., Some(overlay_dir), ..) or \
                 MountConfig::new_read_only(..) for a read-only mount."
            );
        }
        Mode::ReadOnly | Mode::ReadOnlyIfSupported => NfsAdapter::new(vfs).serve(bind_port).await?,
    };

    let port = server_handle.port();

    // `soft` with a bounded timeout so a dead userspace NFS server (e.g. the
    // sidecar crashed) surfaces EIO to clients instead of wedging them in
    // uninterruptible D-state on every access, which a hard mount would. The
    // server is always local, so the usual soft-mount data-loss caveat is moot:
    // if it dies, the mount is gone regardless. timeo is in deciseconds.
    let mut opts = format!(
        "noacl,nolock,soft,timeo=100,retrans=3,vers=3,tcp,port={port},mountport={port},rsize=1048576"
    );
    if read_only {
        opts.push_str(",ro");
    } else {
        opts.push_str(",wsize=1048576");
    }

    #[cfg(target_os = "macos")]
    {
        let mnt = config.mount_point.display().to_string();
        let status = tokio::process::Command::new("mount_nfs")
            .args(["-o", &opts, "localhost:/", &mnt])
            .status()
            .await?;
        if !status.success() {
            server_handle.abort();
            anyhow::bail!("NFS mount failed with exit status {status}");
        }
    }

    #[cfg(target_os = "linux")]
    {
        let mnt = config.mount_point.display().to_string();
        // Probe passwordless sudo first — `mount -t nfs` needs CAP_SYS_ADMIN
        // which isn't available in unprivileged user namespaces, so there's no
        // userspace fallback we can reach for. Fail loudly instead of letting
        // sudo prompt interactively (terrible UX in `pixi run`).
        //
        // TODO(bind-mount): investigate `unshare -Urm` + `mount --bind` as a
        // rootless alternative transport on Linux. That would work in rootless
        // containers where neither FUSE nor sudo is available.
        let probe = tokio::process::Command::new("sudo")
            .args(["-n", "true"])
            .status()
            .await;
        match probe {
            Ok(s) if s.success() => {}
            _ => {
                server_handle.abort();
                return Err(MountError::SudoRequired.into());
            }
        }

        let status = tokio::process::Command::new("sudo")
            .args(["mount", "-t", "nfs", "-o", &opts, "localhost:/", &mnt])
            .status()
            .await?;
        if !status.success() {
            server_handle.abort();
            anyhow::bail!("NFS mount failed with exit status {status}");
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        server_handle.abort();
        anyhow::bail!("NFS mount is not supported on this platform. Use ProjFS on Windows.");
    }

    // Only reachable on the NFS-capable targets; on other platforms the block
    // above diverges, so gate the success tail to avoid unreachable-code errors.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        tracing::info!("mounted via NFS on {}", config.mount_point.display());

        Ok(nfs_adapter::NfsMountHandle {
            mount_point: config.mount_point.clone(),
            server_handle,
            unmounted: false,
        })
    }
}

/// Create an overlay, wiping and retrying transparently on state-version
/// mismatch (internal schema change). Returns a structured error on env-hash
/// mismatch so the caller can decide whether to wipe — the overlay may contain
/// user work. Refuses (does not wipe) on transport mismatch.
///
/// Acquires the directory lock once and carries it through the wipe-and-retry
/// path so no other process can sneak in between the wipe and the reload.
#[cfg(any(feature = "nfs", target_os = "linux", feature = "fuse"))]
fn create_overlay(
    vfs: VirtualFS,
    overlay_dir: &Path,
    env_hash: &str,
    transport: &str,
    overlay_mismatch: OverlayMismatch,
) -> anyhow::Result<overlay_fs::OverlayFS<VirtualFS>> {
    use crate::overlay::{OverlayError, OverlayState};

    // Acquire the lock once, before the first load attempt. The lock handle
    // is passed through both the initial load and the retry so the wipe
    // step is protected.
    let lock = OverlayState::acquire_lock(overlay_dir)
        .map_err(|e| anyhow::anyhow!("failed to acquire overlay lock: {e}"))?;

    let state = match OverlayState::load_with_lock(
        overlay_dir.to_path_buf(),
        env_hash.to_string(),
        transport.to_string(),
        overlay_mismatch,
        lock,
    ) {
        Ok(state) => state,
        Err(OverlayError::EnvHashMismatch {
            expected, found, ..
        }) => {
            return Err(MountError::OverlayEnvHashMismatch {
                expected,
                found,
                overlay_dir: overlay_dir.to_path_buf(),
            }
            .into());
        }
        Err(OverlayError::VersionMismatch { lock, .. }) => {
            tracing::info!("overlay state version changed; wiping and recreating");
            // Lock is still held — safe to wipe without a race.
            if overlay_dir.exists() {
                std::fs::remove_dir_all(overlay_dir)?;
            }
            OverlayState::load_with_lock(
                overlay_dir.to_path_buf(),
                env_hash.to_string(),
                transport.to_string(),
                overlay_mismatch,
                lock,
            )
            .map_err(|e| anyhow::anyhow!("failed to recreate overlay state: {e}"))?
        }
        Err(OverlayError::TransportMismatch {
            expected, found, ..
        }) => {
            return Err(MountError::OverlayTransportMismatch { expected, found }.into());
        }
        Err(e) => anyhow::bail!("failed to load overlay state: {e}"),
    };

    overlay_fs::OverlayFS::wrap(vfs, state)
        .map_err(|e| anyhow::anyhow!("failed to wrap VFS with overlay: {e}"))
}

/// Force unmount a mount point.
///
/// Best-effort cleanup for stale mounts (e.g. after a crash).  The
/// `transport` hint selects the right teardown method:
///
/// | Platform | Transport | Method |
/// |----------|-----------|--------|
/// | Linux | FUSE | `fusermount3 -uz` |
/// | Linux | NFS | `sudo umount -f` (requires passwordless sudo) |
/// | macOS | FUSE / NFS | `umount -f` |
/// | Windows | `ProjFS` | Not yet supported — returns an error |
///
/// Pass [`Transport::Auto`] to use the platform default.
///
/// **NFS on Linux note:** `umount -f` requires `CAP_SYS_ADMIN`.  If
/// passwordless sudo is not available, this will fail.  Consider switching
/// to [`Transport::Fuse`] where possible.
///
/// **`ProjFS` note:** stale `ProjFS` mounts are structurally different — the
/// virtualization context died with the owning process, but hydrated files
/// remain.  Recovery currently requires wiping the directory and remounting.
/// A future version may support re-attaching to an existing virtualization
/// root.
pub fn force_unmount(mount_point: &Path, transport: Transport) -> anyhow::Result<()> {
    let transport = transport.resolve();
    let mnt = mount_point.display().to_string();

    match transport {
        #[cfg(any(target_os = "linux", feature = "fuse"))]
        Transport::Fuse => {
            #[cfg(target_os = "linux")]
            {
                let status = std::process::Command::new("fusermount3")
                    .args(["-uz", &mnt])
                    .status()?;
                if !status.success() {
                    anyhow::bail!("fusermount3 -uz {mnt} failed (exit {status})");
                }
            }
            #[cfg(target_os = "macos")]
            {
                let status = std::process::Command::new("umount")
                    .args(["-f", &mnt])
                    .status()?;
                if !status.success() {
                    anyhow::bail!("umount -f {mnt} failed (exit {status})");
                }
            }
        }
        #[cfg(feature = "nfs")]
        Transport::Nfs => {
            #[cfg(target_os = "macos")]
            {
                let status = std::process::Command::new("umount")
                    .args(["-f", &mnt])
                    .status()?;
                if !status.success() {
                    anyhow::bail!("umount -f {mnt} failed (exit {status})");
                }
            }
            #[cfg(target_os = "linux")]
            {
                let status = std::process::Command::new("sudo")
                    .args(["umount", "-f", &mnt])
                    .status()?;
                if !status.success() {
                    anyhow::bail!(
                        "sudo umount -f {mnt} failed (exit {status}). \
                         NFS force-unmount on Linux requires passwordless sudo."
                    );
                }
            }
        }
        #[cfg(target_os = "windows")]
        Transport::ProjFs => {
            anyhow::bail!(
                "ProjFS stale-mount recovery is not yet supported. \
                 The virtualization context died with the owning process; \
                 hydrated files remain at {mnt}. Remove the directory \
                 manually and remount, or wait for re-attach support."
            );
        }
        _ => {
            anyhow::bail!(
                "force_unmount: transport {transport:?} is not available on this platform"
            );
        }
    }

    #[allow(unreachable_code)]
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rattler_conda_types::package::{PathType, PathsEntry, PathsJson};
    use std::path::PathBuf;

    fn make_paths_json(paths: Vec<&str>) -> PathsJson {
        PathsJson {
            paths: paths
                .into_iter()
                .map(|p| PathsEntry {
                    relative_path: PathBuf::from(p),
                    path_type: PathType::HardLink,
                    prefix_placeholder: None,
                    no_link: false,
                    sha256: None,
                    size_in_bytes: None,
                })
                .collect(),
            paths_version: 1,
        }
    }

    #[test]
    fn test_single_file_at_root() {
        let paths_json = make_paths_json(vec!["foo.txt"]);
        let (mut env_paths, mut dir_indices) = new_empty_tree();
        path_parse(
            &paths_json,
            Path::new("/cache/pkg"),
            None,
            &mut env_paths,
            &mut dir_indices,
        );

        assert_eq!(env_paths.len(), 2); // root + foo.txt
        let root = env_paths[0].as_directory().unwrap();
        assert_eq!(root.children.len(), 1);
        let file = env_paths[root.children[0]].as_file().unwrap();
        assert_eq!(file.file_name, "foo.txt");
        assert_eq!(&*file.cache_base_path, Path::new("/cache/pkg"));
    }

    #[test]
    fn test_nested_directories() {
        let paths_json = make_paths_json(vec!["a/b/c.txt"]);
        let (mut env_paths, mut dir_indices) = new_empty_tree();
        path_parse(
            &paths_json,
            Path::new("/cache/pkg"),
            None,
            &mut env_paths,
            &mut dir_indices,
        );

        // root, dir "a", dir "a/b", file "c.txt"
        assert_eq!(env_paths.len(), 4);

        let root = env_paths[0].as_directory().unwrap();
        assert_eq!(root.children.len(), 1);

        let dir_a = env_paths[root.children[0]].as_directory().unwrap();
        assert_eq!(dir_a.prefix_path, PathBuf::from("./a"));
        assert_eq!(dir_a.children.len(), 1);

        let dir_b = env_paths[dir_a.children[0]].as_directory().unwrap();
        assert_eq!(dir_b.prefix_path, PathBuf::from("./a/b"));
        assert_eq!(dir_b.children.len(), 1);

        let file = env_paths[dir_b.children[0]].as_file().unwrap();
        assert_eq!(file.file_name, "c.txt");
    }

    #[test]
    fn test_directory_dedup() {
        let paths_json = make_paths_json(vec!["lib/foo", "lib/bar"]);
        let (mut env_paths, mut dir_indices) = new_empty_tree();
        path_parse(
            &paths_json,
            Path::new("/cache/pkg"),
            None,
            &mut env_paths,
            &mut dir_indices,
        );

        // root, dir "lib", file "foo", file "bar"
        assert_eq!(env_paths.len(), 4);

        let root = env_paths[0].as_directory().unwrap();
        assert_eq!(root.children.len(), 1); // single lib dir

        let lib_dir = env_paths[root.children[0]].as_directory().unwrap();
        assert_eq!(lib_dir.children.len(), 2); // foo and bar
    }

    #[test]
    fn test_multiple_packages() {
        let pkg1 = make_paths_json(vec!["lib/foo.so"]);
        let pkg2 = make_paths_json(vec!["lib/bar.so"]);

        let (mut env_paths, mut dir_indices) = new_empty_tree();
        path_parse(
            &pkg1,
            Path::new("/cache/pkg1"),
            None,
            &mut env_paths,
            &mut dir_indices,
        );
        path_parse(
            &pkg2,
            Path::new("/cache/pkg2"),
            None,
            &mut env_paths,
            &mut dir_indices,
        );

        // root, dir "lib", file "foo.so", file "bar.so"
        assert_eq!(env_paths.len(), 4);

        let lib_dir = env_paths[1].as_directory().unwrap();
        assert_eq!(lib_dir.children.len(), 2);

        let foo = env_paths[lib_dir.children[0]].as_file().unwrap();
        assert_eq!(&*foo.cache_base_path, Path::new("/cache/pkg1"));

        let bar = env_paths[lib_dir.children[1]].as_file().unwrap();
        assert_eq!(&*bar.cache_base_path, Path::new("/cache/pkg2"));
    }

    #[test]
    fn test_empty_paths_json() {
        let paths_json = make_paths_json(vec![]);
        let (mut env_paths, mut dir_indices) = new_empty_tree();
        path_parse(
            &paths_json,
            Path::new("/cache/pkg"),
            None,
            &mut env_paths,
            &mut dir_indices,
        );

        assert_eq!(env_paths.len(), 1); // root only
        let root = env_paths[0].as_directory().unwrap();
        assert_eq!(root.children.len(), 0);
    }

    fn make_python_info() -> PythonInfo {
        use rattler_conda_types::Version;
        use std::str::FromStr;
        PythonInfo::from_version(
            &Version::from_str("3.11.0").unwrap(),
            None,
            rattler_conda_types::Platform::Linux64,
        )
        .unwrap()
    }

    fn make_entry_points() -> Vec<EntryPoint> {
        use std::str::FromStr;
        vec![
            EntryPoint::from_str("ipython = IPython:start_ipython").unwrap(),
            EntryPoint::from_str("ipython3 = IPython:start_ipython").unwrap(),
        ]
    }

    #[test]
    fn test_entry_points_creates_bin_dir() {
        let (mut env_paths, mut dir_indices) = new_empty_tree();
        let python_info = make_python_info();
        add_entry_points(
            &make_entry_points(),
            "/prefix",
            &python_info,
            &mut env_paths,
            &mut dir_indices,
        );

        // root + bin dir + 2 files
        assert!(dir_indices.contains_key(&PathBuf::from("./bin")));
        let root = env_paths[0].as_directory().unwrap();
        assert_eq!(root.children.len(), 1); // bin dir
    }

    #[test]
    fn test_entry_points_adds_files() {
        let (mut env_paths, mut dir_indices) = new_empty_tree();
        let python_info = make_python_info();
        add_entry_points(
            &make_entry_points(),
            "/prefix",
            &python_info,
            &mut env_paths,
            &mut dir_indices,
        );

        let bin_idx = dir_indices[&PathBuf::from("./bin")];
        let bin_dir = env_paths[bin_idx].as_directory().unwrap();
        assert_eq!(bin_dir.children.len(), 2);

        let names: Vec<_> = bin_dir
            .children
            .iter()
            .map(|&i| env_paths[i].file_name().to_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"ipython".to_string()));
        assert!(names.contains(&"ipython3".to_string()));
    }

    #[test]
    fn test_entry_points_virtual_content() {
        let (mut env_paths, mut dir_indices) = new_empty_tree();
        let python_info = make_python_info();
        add_entry_points(
            &make_entry_points(),
            "/prefix",
            &python_info,
            &mut env_paths,
            &mut dir_indices,
        );

        let bin_idx = dir_indices[&PathBuf::from("./bin")];
        let bin_dir = env_paths[bin_idx].as_directory().unwrap();
        let file = env_paths[bin_dir.children[0]].as_file().unwrap();

        let content = file
            .virtual_content
            .as_ref()
            .expect("should have virtual content");
        let text = std::str::from_utf8(content).unwrap();
        assert!(
            text.contains("#!/prefix/bin/python3.11"),
            "shebang missing: {text}"
        );
        assert!(
            text.contains("from IPython import"),
            "import missing: {text}"
        );
        assert!(
            text.contains("start_ipython()"),
            "function call missing: {text}"
        );
    }

    #[test]
    fn test_entry_points_dedup_bin_dir() {
        // Create a tree that already has bin/ from another package
        let pkg = make_paths_json(vec!["bin/existing"]);
        let (mut env_paths, mut dir_indices) = new_empty_tree();
        path_parse(
            &pkg,
            Path::new("/cache/pkg"),
            None,
            &mut env_paths,
            &mut dir_indices,
        );

        let bin_idx = dir_indices[&PathBuf::from("./bin")];
        let before_children = env_paths[bin_idx].as_directory().unwrap().children.len();
        assert_eq!(before_children, 1); // just "existing"

        let python_info = make_python_info();
        add_entry_points(
            &make_entry_points(),
            "/prefix",
            &python_info,
            &mut env_paths,
            &mut dir_indices,
        );

        // bin dir should now have 3 children (existing + ipython + ipython3), not a new bin dir
        let bin_dir = env_paths[bin_idx].as_directory().unwrap();
        assert_eq!(bin_dir.children.len(), 3);

        // Root should still only have 1 child (the single bin dir)
        let root = env_paths[0].as_directory().unwrap();
        assert_eq!(root.children.len(), 1);
    }

    // --- noarch Python path rewriting tests ---

    #[test]
    fn test_noarch_python_rewrites_site_packages() {
        let paths_json = make_paths_json(vec![
            "site-packages/foo/__init__.py",
            "site-packages/foo/bar.py",
        ]);
        let python_info = make_python_info(); // python 3.11
        let (mut env_paths, mut dir_indices) = new_empty_tree();
        path_parse(
            &paths_json,
            Path::new("/cache/pkg"),
            Some(&python_info),
            &mut env_paths,
            &mut dir_indices,
        );

        // Should have: lib/python3.11/site-packages/foo/ directory structure
        assert!(dir_indices.contains_key(&PathBuf::from("./lib")));
        assert!(dir_indices.contains_key(&PathBuf::from("./lib/python3.11")));
        assert!(dir_indices.contains_key(&PathBuf::from("./lib/python3.11/site-packages")));
        assert!(dir_indices.contains_key(&PathBuf::from("./lib/python3.11/site-packages/foo")));
        // Should NOT have bare site-packages at root
        assert!(!dir_indices.contains_key(&PathBuf::from("./site-packages")));
    }

    #[test]
    fn test_noarch_python_rewrites_python_scripts() {
        let paths_json = make_paths_json(vec!["python-scripts/mycmd"]);
        let python_info = make_python_info();
        let (mut env_paths, mut dir_indices) = new_empty_tree();
        path_parse(
            &paths_json,
            Path::new("/cache/pkg"),
            Some(&python_info),
            &mut env_paths,
            &mut dir_indices,
        );

        // Should appear under bin/
        assert!(dir_indices.contains_key(&PathBuf::from("./bin")));
        let bin_idx = dir_indices[&PathBuf::from("./bin")];
        let bin_dir = env_paths[bin_idx].as_directory().unwrap();
        assert_eq!(bin_dir.children.len(), 1);
        let file = env_paths[bin_dir.children[0]].as_file().unwrap();
        assert_eq!(file.file_name, "mycmd");
    }

    #[test]
    fn test_noarch_python_preserves_cache_path() {
        let paths_json = make_paths_json(vec!["site-packages/foo/bar.py"]);
        let python_info = make_python_info();
        let (mut env_paths, mut dir_indices) = new_empty_tree();
        path_parse(
            &paths_json,
            Path::new("/cache/noarch-pkg"),
            Some(&python_info),
            &mut env_paths,
            &mut dir_indices,
        );

        // Find bar.py
        let foo_idx = dir_indices[&PathBuf::from("./lib/python3.11/site-packages/foo")];
        let foo_dir = env_paths[foo_idx].as_directory().unwrap();
        let file = env_paths[foo_dir.children[0]].as_file().unwrap();

        // cache_base_path points to the package cache
        assert_eq!(&*file.cache_base_path, Path::new("/cache/noarch-pkg"));
        // cache_prefix_path overrides to original on-disk location
        assert_eq!(
            file.cache_prefix_path.as_deref(),
            Some(Path::new("./site-packages/foo"))
        );
    }

    #[test]
    fn test_noarch_non_rewritten_paths_unchanged() {
        // Files not under site-packages/ or python-scripts/ should be unchanged
        let paths_json = make_paths_json(vec!["share/data/file.txt"]);
        let python_info = make_python_info();
        let (mut env_paths, mut dir_indices) = new_empty_tree();
        path_parse(
            &paths_json,
            Path::new("/cache/pkg"),
            Some(&python_info),
            &mut env_paths,
            &mut dir_indices,
        );

        assert!(dir_indices.contains_key(&PathBuf::from("./share")));
        assert!(dir_indices.contains_key(&PathBuf::from("./share/data")));
        let data_idx = dir_indices[&PathBuf::from("./share/data")];
        let file = env_paths[env_paths[data_idx].as_directory().unwrap().children[0]]
            .as_file()
            .unwrap();
        assert_eq!(file.file_name, "file.txt");
        // No cache_prefix_path override needed
        assert!(file.cache_prefix_path.is_none());
    }

    #[test]
    fn test_non_noarch_no_rewrite() {
        // Without python_info, site-packages/ stays as-is
        let paths_json = make_paths_json(vec!["site-packages/foo/bar.py"]);
        let (mut env_paths, mut dir_indices) = new_empty_tree();
        path_parse(
            &paths_json,
            Path::new("/cache/pkg"),
            None,
            &mut env_paths,
            &mut dir_indices,
        );

        assert!(dir_indices.contains_key(&PathBuf::from("./site-packages")));
        assert!(!dir_indices.contains_key(&PathBuf::from("./lib")));
    }

    /// Minimal v6 lockfile with 2 conda packages (non-alphabetical order)
    /// used exclusively for the env hash golden test. Unlike the full
    /// test-data/rattler-vfs/pixi.lock, this fixture never changes when
    /// upstream dependencies are bumped.
    const GOLDEN_LOCKFILE: &str = "\
version: 6
environments:
  default:
    channels:
    - url: https://prefix.dev/conda-forge/
    packages:
      linux-64:
      - conda: https://prefix.dev/conda-forge/noarch/tzdata-2025c-hc9c84f9_1.conda
      - conda: https://prefix.dev/conda-forge/noarch/iniconfig-2.3.0-pyhd8ed1ab_0.conda
packages:
- conda: https://prefix.dev/conda-forge/noarch/tzdata-2025c-hc9c84f9_1.conda
  sha256: 1d30098909076af33a35017eed6f2953af1c769e273a0626a04722ac4acaba3c
  md5: ad659d0a2b3e47e38d829aa8cad2d610
  license: LicenseRef-Public-Domain
  size: 119135
  timestamp: 1767016325805
- conda: https://prefix.dev/conda-forge/noarch/iniconfig-2.3.0-pyhd8ed1ab_0.conda
  sha256: e1a9e3b1c8fe62dc3932a616c284b5d8cbe3124bbfbedcf4ce5c828cb166ee19
  md5: 9614359868482abba1bd15ce465e3c42
  depends:
  - python >=3.10
  license: MIT
  license_family: MIT
  size: 13387
  timestamp: 1760831448842
";

    #[test]
    fn test_env_hash_stability() {
        // Golden test: if this fails, either the canonical form drifted
        // accidentally (fix the drift) or ENV_HASH_SCHEMA_VERSION was bumped
        // intentionally (update the expected hash below).
        let lockfile = rattler_lock::LockFile::from_reader(GOLDEN_LOCKFILE.as_bytes(), None)
            .expect("golden lockfile should parse");

        let hash =
            compute_env_hash(&lockfile, "default", Platform::Linux64).expect("hash should succeed");

        // To update: run `cargo test -p rattler_vfs test_env_hash_stability`
        // and copy the "got" value here.
        assert_eq!(
            hash, "sha256:e2822c5c31a5cc682a0eb82ef1eb867eec1cde2373bf159a37c7261a23011ecd",
            "env hash drifted. If intentional (e.g. ENV_HASH_SCHEMA_VERSION bumped), \
             update this golden value. If accidental, investigate what changed in the \
             canonical form."
        );
    }
}
