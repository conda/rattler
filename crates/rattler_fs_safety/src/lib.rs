//! Symlink-resistant filesystem helpers shared across rattler
//! crates.
//!
//! The helpers here all operate against a [`cap_std::fs::Dir`]
//! capability. cap-std refuses path-component traversal that
//! escapes the directory, so the only remaining attack surface is
//! the **final component** of a path -- the helpers wrap that with
//! explicit symlink rejection so callers don't have to remember.
//!
//! Two flavors of opener/writer are exposed:
//!
//! * `*_in(&Dir, …)` -- the primary form. Callers open the parent
//!   directory once via [`Dir::open_ambient_dir`] and reuse the
//!   handle. The whole open / write / fchmod / rename sequence
//!   stays anchored to that one capability, so an attacker can't
//!   race a symlink swap of an intermediate path component
//!   between successive helper calls.
//! * `*_in_dir(&Path, …)` -- convenience wrapper that opens the
//!   `Dir` itself on every call. Fine for one-shot writes;
//!   prefer the `_in` form inside hot loops or when several
//!   helpers act on the same directory.
//!
//! The full helper set:
//!
//! * [`open_no_follow_in`] / [`open_no_follow`] -- opens an
//!   existing or newly-created regular file inside the parent,
//!   refusing to follow a symlink at the final component. Used
//!   for `.lock` files and any short-lived metadata file the
//!   daemon writes inside a shared cache root.
//! * [`atomic_write_in`] / [`atomic_write_in_dir`] -- writes
//!   `bytes` to a temp file in the parent, optionally `fchmod`s
//!   the still-open fd to `mode`, then renames over `name`.
//!   Caller never sees a half-written file at the final path;
//!   the chmod happens on the fd, not on the path, so a
//!   co-tenant can't race a symlink between create and chmod.
//! * [`validate_relative_inside`] -- lexically normalises a
//!   candidate relative path and refuses anything that would
//!   escape `root`. For use against archive-supplied
//!   `paths.json::relative_path`, menuinst manifest dest paths,
//!   and similar attacker-influenced strings *before* they're
//!   joined to a trusted root.

use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};

// Re-exported so callers can build [`open_no_follow`] arguments
// (and use cap-std `Dir`s for their own anchored writes) without
// taking a direct dependency on cap-std.
pub use cap_std::ambient_authority;
pub use cap_std::fs::{Dir, DirBuilder, File, OpenOptions, OpenOptionsExt, Permissions};
#[cfg(unix)]
pub use cap_std::fs::{DirBuilderExt, PermissionsExt};

#[cfg(windows)]
mod windows_sec;

/// Open `name` inside `dir` while refusing to follow a symlink at
/// the final component.
///
/// `dir` is a [`Dir`] capability, so any `..` or absolute-path
/// traversal in `name` is rejected by cap-std before we look. The
/// symlink rejection guards the *final* component: even a
/// same-directory symlink (e.g. `<entry>.lock` →
/// `<other_entry>.lock`) is refused, since the caller asked to
/// open `name`, not whatever `name` redirects to.
///
/// The "don't follow" decision is pushed into the open syscall
/// itself -- cap-std translates `FollowSymlinks::No` into
/// `O_NOFOLLOW` on POSIX and the reparse-point-aware equivalent
/// on Windows -- so the check and the open are a single atomic
/// kernel call. There is no TOCTOU window where a co-tenant could
/// swap a regular file for a symlink between an `lstat` and the
/// `open`.
///
/// Symlink refusals are remapped to
/// [`io::ErrorKind::PermissionDenied`] (the kernel reports them
/// as `ELOOP`/`EMLINK`/`EFTYPE` depending on platform) so callers
/// don't have to branch on platform-specific error kinds.
pub fn open_no_follow_in(dir: &Dir, name: &OsStr, opts: &OpenOptions) -> io::Result<File> {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};

    // Reject multi-component names. cap-std's path walk only
    // applies `FollowSymlinks::No` to the *final* component; an
    // intermediate component that happens to be a symlink is
    // dereferenced, which would let a co-tenant race the
    // intermediate target. By forcing `name` to a single
    // component we collapse the whole open into one `openat`
    // syscall against `dir` and there are no intermediate
    // components to race.
    ensure_single_component(name)?;

    // Clone the caller's options so the rest of their flags
    // (read/write/create/...) are preserved while we force the
    // final-component symlink behavior off.
    let mut opts = opts.clone();
    opts.follow(FollowSymlinks::No);

    dir.open_with(name, &opts).map_err(|err| {
        if is_symlink_refusal(&err) {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("refusing to follow symlink: {name:?}"),
            )
        } else {
            err
        }
    })
}

/// Reject any `name` that isn't a single, normal path component.
///
/// The helpers in this crate are designed to operate on the final
/// component of a path: that's the only component cap-std walks
/// with `O_NOFOLLOW`, and the only one `renameat` doesn't
/// follow-through-symlinks on the destination side. A caller who
/// passes `"a/b"` would re-introduce both of those races. The
/// check also catches empty names, `.`, `..`, absolute paths, and
/// (on Windows) backslash separators by virtue of how
/// `Path::components` classifies them.
fn ensure_single_component(name: &OsStr) -> io::Result<()> {
    let mut comps = Path::new(name).components();
    let first = comps.next();
    let rest = comps.next();
    match (first, rest) {
        (Some(Component::Normal(c)), None) if c == name => Ok(()),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("name must be a single path component: {name:?}"),
        )),
    }
}

/// Path-based wrapper around [`open_no_follow_in`] that opens
/// `parent` as a [`Dir`] capability for a single call. Use the
/// `_in` form when issuing several anchored operations against
/// the same directory.
pub fn open_no_follow(parent: &Path, name: &OsStr, opts: &OpenOptions) -> io::Result<File> {
    let dir = Dir::open_ambient_dir(parent, ambient_authority())?;
    open_no_follow_in(&dir, name, opts)
}

/// `DirBuilder` configured for owner-only mode (`0o700`) on Unix,
/// defaults elsewhere. Used to atomically `mkdirat` a private
/// tempdir so no `umask`-dependent window exposes the entry.
fn private_dir_builder() -> DirBuilder {
    #[cfg(unix)]
    {
        let mut b = DirBuilder::new();
        b.mode(0o700);
        b
    }
    #[cfg(not(unix))]
    {
        DirBuilder::new()
    }
}

/// True if `err` is the kernel reporting that `O_NOFOLLOW` (or its
/// platform equivalent) tripped on a symlink at the final component.
///
/// The error codes match cap-primitives' own classification in
/// `open_unchecked.rs` so the mapping stays in sync with whatever
/// cap-std considers a "symlink refusal".
#[cfg(unix)]
fn is_symlink_refusal(err: &io::Error) -> bool {
    let Some(raw) = err.raw_os_error() else {
        return false;
    };
    #[cfg(not(any(target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd")))]
    {
        raw == libc::ELOOP
    }
    #[cfg(any(target_os = "freebsd", target_os = "dragonfly"))]
    {
        raw == libc::EMLINK
    }
    #[cfg(target_os = "netbsd")]
    {
        raw == libc::EFTYPE
    }
}

/// On Windows we match `ERROR_STOPPED_ON_SYMLINK` and
/// `ERROR_CANT_ACCESS_FILE`, the two reparse-specific codes the
/// kernel raises when the final-component reparse check trips.
/// Other errors pass through unchanged so callers still see e.g.
/// `NotFound` or `AlreadyExists` with their natural kind.
#[cfg(windows)]
fn is_symlink_refusal(err: &io::Error) -> bool {
    windows_sec::is_symlink_refusal(err)
}

#[cfg(not(any(unix, windows)))]
fn is_symlink_refusal(_err: &io::Error) -> bool {
    false
}

/// Atomically write `bytes` to `name` inside `dir`, replacing any
/// existing entry on success. If `mode` is `Some` (Unix only),
/// the file is `fchmod`-ed to that mode via the still-open fd
/// before publish. If `mode` is `None`, the file is published
/// with owner-only mode `0o600`.
///
/// **TOCTOU model.** The helper is robust against a co-tenant
/// with write access to `dir`. All tempfile work happens inside
/// a per-call private subdirectory of `dir`: on Unix it is
/// `mkdirat`-ed with mode `0o700` and stat-verified to belong to
/// the current effective uid; on Windows it is created via
/// `NtCreateFile` with `OBJECT_ATTRIBUTES.RootDirectory =
/// dir.HANDLE` and an explicit `PROTECTED` owner-only
/// `SECURITY_DESCRIPTOR`, so placement and DACL are atomic with
/// respect to both intermediate-component substitution and
/// parent DACL inheritance. Either way, no other process can
/// populate or substitute the tempfile. The final publish is an
/// fd-anchored rename -- `renameat` on Unix,
/// `SetFileInformationByHandle(FileRenameInfoEx)` with
/// `RootDirectory = dir.HANDLE` on Windows -- so the destination
/// is identified entirely by the caller's directory handle plus
/// the single-component `name`, with no path resolution that an
/// attacker could redirect.
///
/// On Windows the `mode` parameter is ignored. The published
/// file always carries a `PROTECTED` owner-only DACL, applied
/// atomically at create time by `NtCreateFile` -- there is no
/// post-write permission tweak and no window during which the
/// file is visible with looser access.
///
/// Failure modes leave `dir/name` untouched. The temp directory
/// is cleaned up either explicitly after the rename (best
/// effort) or by [`TempDir`]'s `Drop` if any earlier step
/// failed.
pub fn atomic_write_in(dir: &Dir, name: &OsStr, bytes: &[u8], mode: Option<u32>) -> io::Result<()> {
    use std::io::Write;

    // Reject multi-component names: a `name` like `"foo/bar"`
    // would make the final `renameat` traverse `foo`, which the
    // kernel follows through any symlink a co-tenant placed
    // there. The helper's contract is "name is a single
    // component within `dir`".
    ensure_single_component(name)?;

    let tmp = TempDir::new_in(dir, ".rattler-write-", &private_dir_builder())?;

    // Create the payload anchored to `tmp`'s handle so neither
    // the directory traversal nor the create itself goes through
    // a path the kernel re-resolves.
    //
    // * Unix: `tmp.open_with` is `openat(tmp_fd, "payload",
    //   O_CREAT|O_EXCL|O_RDWR, 0o600)`.
    // * Windows: `create_owner_only_file` uses `NtCreateFile`
    //   with `OBJECT_ATTRIBUTES.RootDirectory = tmp.HANDLE`,
    //   `FILE_CREATE`, and a `PROTECTED` owner-only
    //   `SECURITY_DESCRIPTOR`. The access mask requests
    //   `FILE_GENERIC_WRITE | DELETE` so we can write bytes and
    //   then publish via the fd-anchored rename without
    //   reopening.
    let payload_name = OsStr::new("payload");
    #[cfg(windows)]
    let mut payload = windows_sec::create_owner_only_file(&tmp, payload_name)?;
    #[cfg(not(windows))]
    let mut payload = {
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        opts.mode(0o600);
        tmp.open_with(payload_name, &opts)?
    };

    payload.write_all(bytes)?;
    payload.sync_all()?;

    #[cfg(unix)]
    if let Some(mode) = mode {
        // `fchmod` on the still-open fd -- no path resolution,
        // no race window.
        payload.set_permissions(Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = mode;

    // Atomically publish.
    //
    // On Windows we use a fd-anchored rename via
    // `SetFileInformationByHandle(FileRenameInfoEx)` with
    // `RootDirectory = dir.HANDLE` and `Flags =
    // REPLACE_IF_EXISTS | POSIX_SEMANTICS`. The kernel never
    // resolves the destination by path, so an attacker who
    // substitutes anything along `dir`'s ambient path cannot
    // redirect the publish. `payload` is held open across the
    // call (`DELETE` access in its open mask lets that work).
    //
    // On Unix `tmp.rename(payload_name, dir, name)` is already
    // an fd-anchored cross-directory `renameat`.
    #[cfg(windows)]
    {
        windows_sec::rename_via_handle(&payload, dir, name)?;
        drop(payload);
    }
    #[cfg(not(windows))]
    {
        drop(payload);
        tmp.rename(payload_name, dir, name)?;
    }

    // The rename above is the linearization point. Drop of
    // `tmp` runs the best-effort `remove_open_dir_all` cleanup;
    // any failure there is silently swallowed because the
    // atomic write has already succeeded and callers must not
    // see a spurious error that would prompt a retry.
    drop(tmp);

    Ok(())
}

/// A directory created inside a parent `Dir`, accessible only
/// through this handle and removed on `Drop`.
///
/// `TempDir` is the reusable building block behind
/// [`atomic_write_in`]: a private workspace where attacker-
/// uncontrolled tempfiles can be created and renamed out from.
/// Compared to `cap_tempfile::TempDir`, it locks down both
/// placement and permissions atomically at creation:
///
/// * **Unix:** `mkdirat` via the caller-supplied [`DirBuilder`]
///   (pass `DirBuilder::new().mode(0o700)` for owner-only). The
///   directory is then stat-verified post-`openat` to confirm
///   `meta.uid() == geteuid()`, which catches a co-tenant who
///   renamed our subdirectory away between `mkdirat` and
///   `openat`: only `root` can `chown`, so the substituted dir
///   fails the check.
/// * **Windows:** `NtCreateFile` with `FILE_CREATE`,
///   `RootDirectory = parent.HANDLE`, and an explicit
///   `SECURITY_DESCRIPTOR` carrying `SE_DACL_PROTECTED` and a
///   single inheritable allow-ACE for the current user's SID.
///   The kernel resolves the name relative to the parent handle
///   (no path resolution, no intermediate-component race) and
///   applies the security descriptor as part of the create
///   (no inherit-then-harden window). A `GetSecurityInfo`
///   ownership check on the resulting handle backs the
///   filesystem layer (FAT ignores ACLs; this catches that).
///
/// The `prefix` argument is prepended to a per-call random
/// suffix to form the directory name; choose a value that
/// identifies the call site so leftover directories (if cleanup
/// ever fails) are traceable. Random-suffix collisions are
/// retried with a fresh name, bounded so a co-tenant who guesses
/// the name space cannot indefinitely deny service.
///
/// `TempDir` derefs to [`Dir`], so the usual `open_with` /
/// `rename` / `create_dir` methods are available directly.
pub struct TempDir {
    /// `None` once `close` has been called, so `Drop` becomes a
    /// no-op.
    dir: Option<Dir>,
}

impl TempDir {
    /// Create a new temp directory inside `parent`. The directory
    /// is named `{prefix}{random_suffix}` and the [`DirBuilder`]
    /// controls how the underlying `mkdirat` is performed (pass
    /// `DirBuilder::new().mode(0o700)` on Unix to get a private
    /// directory).
    pub fn new_in(parent: &Dir, prefix: &str, builder: &DirBuilder) -> io::Result<Self> {
        // `builder` is only consulted on platforms whose
        // capability layer takes the mkdir mode at creation time
        // (Unix). On Windows the equivalent -- the per-call
        // SECURITY_DESCRIPTOR -- is built inside
        // `windows_sec::create_owner_only_subdir`.
        #[cfg(windows)]
        let _ = builder;
        // Bounded retries on `AlreadyExists` so a co-tenant who
        // can predict our naming scheme can't hang us
        // indefinitely by pre-creating the next ~N names.
        for _ in 0..16 {
            let name = random_suffix(prefix);

            #[cfg(windows)]
            let attempt =
                windows_sec::create_owner_only_subdir(parent, std::ffi::OsStr::new(&name));

            #[cfg(not(windows))]
            let attempt = parent.create_dir_with(&name, builder).and_then(|()| {
                parent.open_dir(&name).inspect_err(|_| {
                    // We created the directory but couldn't open
                    // it. Best-effort path-based cleanup so we
                    // don't leak the entry. If a co-tenant
                    // substituted the entry before our
                    // `remove_dir`, the rmdir either succeeds
                    // (their dir was empty) or fails silently (we
                    // leak their dir, not ours).
                    let _ = parent.remove_dir(&name);
                })
            });

            match attempt {
                Ok(dir) => {
                    Self::verify_owner(&dir).inspect_err(|_| {
                        let _ = dir.try_clone().and_then(Dir::remove_open_dir_all);
                        #[cfg(not(windows))]
                        let _ = parent.remove_dir_all(&name);
                    })?;
                    return Ok(Self { dir: Some(dir) });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a tempdir name after 16 attempts",
        ))
    }

    /// Confirm the opened directory is still owned by the current
    /// process identity. Catches the substitution race between
    /// `mkdirat` and `openat` where a co-tenant with write access
    /// to the parent renames our subdirectory away and puts theirs
    /// in its place.
    ///
    /// On Unix this is `geteuid() == meta.uid()` -- `mkdir` always
    /// sets the owner to the caller's uid, and only `root` can
    /// `chown`. On Windows it's `EqualSid(owner, our_token_sid)`
    /// read via `GetSecurityInfo` on the open handle, which has
    /// the analogous property: a freshly created object's owner
    /// SID is the creator's token SID.
    #[cfg(unix)]
    fn verify_owner(dir: &Dir) -> io::Result<()> {
        use cap_std::fs::MetadataExt;
        let meta = dir.dir_metadata()?;
        // SAFETY: `geteuid` takes no arguments and is documented
        // by POSIX to always succeed.
        let our_uid = unsafe { libc::geteuid() };
        if meta.uid() != our_uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "tempdir was substituted between mkdir and open",
            ));
        }
        Ok(())
    }

    #[cfg(windows)]
    fn verify_owner(dir: &Dir) -> io::Result<()> {
        windows_sec::verify_owner(dir)
    }

    #[cfg(not(any(unix, windows)))]
    fn verify_owner(_dir: &Dir) -> io::Result<()> {
        Ok(())
    }

    /// Consume the handle and remove the directory (including
    /// any remaining contents).
    ///
    /// Use this when cleanup failure matters to the caller. The
    /// implicit `Drop` swallows errors, which is the right
    /// behavior when the meaningful work has already happened
    /// (a successful `rename` out of the directory, say), but
    /// the wrong behavior when the directory's removal is the
    /// thing the caller is waiting on.
    pub fn close(mut self) -> io::Result<()> {
        match self.dir.take() {
            Some(dir) => dir.remove_open_dir_all(),
            None => Ok(()),
        }
    }
}

impl std::ops::Deref for TempDir {
    type Target = Dir;
    fn deref(&self) -> &Dir {
        self.dir
            .as_ref()
            .expect("TempDir accessed after close consumed it")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Some(dir) = self.dir.take() {
            let _ = dir.remove_open_dir_all();
        }
    }
}

/// Build a per-call tempdir name as `{prefix}{hash}-{counter}`,
/// where `hash` is a 64-bit digest of the process id, a
/// sub-second timestamp, and a monotonic counter; `counter` is
/// also appended visibly so leftover directories (if cleanup
/// ever fails) can be traced back to a specific call.
///
/// The hash uses a [`RandomState`] cached in a process-wide
/// [`OnceLock`]. The seed is drawn from the OS RNG on first use
/// and reused for every subsequent call: a fresh
/// `RandomState::new()` per call would draw entropy
/// unnecessarily, while a fixed-key [`DefaultHasher`] would let
/// a co-tenant who knows the Rust version compute our hashes
/// from the (predictable) `pid`/`nanos`/`counter` inputs. The
/// cached seed keeps the function deterministic *within* the
/// process while making the suffix unpredictable from outside.
fn random_suffix(prefix: &str) -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::OnceLock;

    static SEED: OnceLock<RandomState> = OnceLock::new();
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut h = SEED.get_or_init(RandomState::new).build_hasher();
    h.write_u32(std::process::id());
    h.write_u32(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos()),
    );
    h.write_u64(counter);
    format!("{prefix}{:016x}-{counter:x}", h.finish())
}

/// Path-based wrapper around [`atomic_write_in`] that opens
/// `parent` as a [`Dir`] capability for a single call. Use the
/// `_in` form when issuing several anchored operations against
/// the same directory.
pub fn atomic_write_in_dir(
    parent: &Path,
    name: &OsStr,
    bytes: &[u8],
    mode: Option<u32>,
) -> io::Result<()> {
    let dir = Dir::open_ambient_dir(parent, ambient_authority())?;
    atomic_write_in(&dir, name, bytes, mode)
}

/// Lexically normalise `candidate` and verify the result lives
/// strictly inside `root`. Returns the normalised path on
/// success, an [`io::ErrorKind::PermissionDenied`] error
/// otherwise.
///
/// "Lexically" means we don't touch the filesystem -- this is for
/// pre-validation of attacker-supplied relative paths (archive
/// `paths.json::relative_path`, menuinst manifest dest, etc.)
/// before they're handed to a higher-trust opener. A `..`
/// component, an absolute path, an empty component, or anything
/// that would resolve outside `root` after lexical normalisation
/// is rejected.
///
/// **Lexical only.** This check guarantees nothing about
/// *intermediate* path components on disk: a validated
/// `pkg/bin/exe` can still escape `root` if `pkg/` happens to be
/// a symlink pointing at `/etc`. Open the result through a
/// [`Dir`] capability rooted at `root` (e.g. via
/// [`open_no_follow_in`], or by walking with cap-std's `Dir`
/// methods) so cap-std refuses any symlink encountered along the
/// way. Passing the joined `PathBuf` directly to
/// [`std::fs::File::open`] *does not* close that gap.
pub fn validate_relative_inside(root: &Path, candidate: &Path) -> io::Result<PathBuf> {
    if candidate.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("absolute path rejected: {candidate:?}"),
        ));
    }

    let mut normalised = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => normalised.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalised.pop() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!("path escapes root: {candidate:?}"),
                    ));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("absolute or prefix component rejected: {candidate:?}"),
                ));
            }
        }
    }

    if normalised.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("empty relative path rejected: {candidate:?}"),
        ));
    }

    Ok(root.join(normalised))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn td() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn open_no_follow_opens_regular_file() {
        let root = td();
        std::fs::write(root.path().join("hello"), b"hi").unwrap();
        let file = open_no_follow(
            root.path(),
            OsStr::new("hello"),
            OpenOptions::new().read(true),
        )
        .unwrap();
        let meta = file.metadata().unwrap();
        assert!(meta.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn open_no_follow_refuses_symlink() {
        let root = td();
        std::fs::write(root.path().join("real"), b"x").unwrap();
        std::os::unix::fs::symlink("real", root.path().join("link")).unwrap();

        let err = open_no_follow(
            root.path(),
            OsStr::new("link"),
            OpenOptions::new().read(true),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
    }

    #[test]
    fn open_no_follow_creates_when_options_allow() {
        let root = td();
        let file = open_no_follow(
            root.path(),
            OsStr::new("new"),
            OpenOptions::new().write(true).create(true),
        )
        .unwrap();
        drop(file);
        assert!(root.path().join("new").is_file());
    }

    #[test]
    fn atomic_write_replaces_atomically() {
        let root = td();
        let target = root.path().join("entry");
        std::fs::write(&target, b"old").unwrap();

        atomic_write_in_dir(root.path(), OsStr::new("entry"), b"new", None).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_defaults_to_owner_only_mode() {
        use std::os::unix::fs::PermissionsExt;
        let root = td();
        atomic_write_in_dir(root.path(), OsStr::new("entry"), b"x", None).unwrap();
        let mode = std::fs::metadata(root.path().join("entry"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_leaves_no_temp_subdirs() {
        let root = td();
        atomic_write_in_dir(root.path(), OsStr::new("entry"), b"payload", None).unwrap();

        let entries: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("entry")]);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_applies_mode_via_fchmod() {
        use std::os::unix::fs::PermissionsExt;
        let root = td();
        atomic_write_in_dir(root.path(), OsStr::new("entry"), b"x", Some(0o600)).unwrap();
        let mode = std::fs::metadata(root.path().join("entry"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn helpers_reject_multi_component_names() {
        // A name containing a separator would re-introduce
        // path-traversal TOCTOU on intermediate components: cap-std
        // only NOFOLLOWs the final component, and `renameat`
        // follows intermediate symlinks on the destination side.
        // Both entry points must refuse such names.
        let root = td();
        let dir = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();

        let cases: &[&OsStr] = &[
            OsStr::new("a/b"),
            OsStr::new(""),
            OsStr::new("."),
            OsStr::new(".."),
            OsStr::new("./foo"),
        ];
        for name in cases {
            let err = open_no_follow_in(&dir, name, OpenOptions::new().read(true).create(true))
                .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "open: {name:?}");

            let err = atomic_write_in(&dir, name, b"x", None).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "write: {name:?}");
        }
    }

    #[test]
    fn dir_anchored_helpers_share_one_capability() {
        // Smoke test: open the parent `Dir` once and reuse it for
        // both an atomic write and a no-follow open. Demonstrates
        // the intended `*_in` calling pattern and proves the
        // `Dir` capability survives across helper calls.
        use std::io::Read;
        let root = td();
        let dir = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();

        atomic_write_in(&dir, OsStr::new("entry"), b"payload", None).unwrap();

        let mut file =
            open_no_follow_in(&dir, OsStr::new("entry"), OpenOptions::new().read(true)).unwrap();
        let mut got = Vec::new();
        file.read_to_end(&mut got).unwrap();
        assert_eq!(got, b"payload");
    }

    #[test]
    fn validate_accepts_relative_descendant() {
        let root = Path::new("/cache");
        let p = validate_relative_inside(root, Path::new("a/b/c")).unwrap();
        assert_eq!(p, Path::new("/cache/a/b/c"));
    }

    #[test]
    fn validate_collapses_curdir() {
        let root = Path::new("/cache");
        let p = validate_relative_inside(root, Path::new("./a/./b")).unwrap();
        assert_eq!(p, Path::new("/cache/a/b"));
    }

    #[test]
    fn validate_allows_balanced_parent() {
        let root = Path::new("/cache");
        let p = validate_relative_inside(root, Path::new("a/../b")).unwrap();
        assert_eq!(p, Path::new("/cache/b"));
    }

    #[test]
    fn validate_rejects_absolute() {
        let err =
            validate_relative_inside(Path::new("/cache"), Path::new("/etc/passwd")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn validate_rejects_escape() {
        let err =
            validate_relative_inside(Path::new("/cache"), Path::new("../etc/passwd")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn validate_rejects_deep_escape() {
        let err =
            validate_relative_inside(Path::new("/cache"), Path::new("a/b/../../../x")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn validate_rejects_empty() {
        let err = validate_relative_inside(Path::new("/cache"), Path::new("")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn validate_rejects_curdir_only() {
        let err = validate_relative_inside(Path::new("/cache"), Path::new(".")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[cfg(windows)]
    #[test]
    fn windows_atomic_write_published_file_is_owner_protected() {
        let root = td();
        atomic_write_in_dir(root.path(), OsStr::new("entry"), b"payload", None).unwrap();

        // The file is created via NtCreateFile with an explicit
        // PROTECTED owner-only SECURITY_DESCRIPTOR, so both the
        // owner SID and the SE_DACL_PROTECTED bit survive the
        // rename onto the (potentially permissive) target dir.
        let file = std::fs::File::open(root.path().join("entry")).unwrap();
        let owner = super::windows_sec::read_owner_sid(&file).unwrap();
        let ours = super::windows_sec::owner_sid().unwrap();
        assert_eq!(owner.as_slice(), ours);
        assert!(super::windows_sec::dacl_is_protected(&file).unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn windows_tempdir_dir_is_owner_protected() {
        // The tempdir is the load-bearing object: its
        // SE_DACL_PROTECTED bit is what stops the parent's
        // ambient ACEs from leaking into the payload's
        // inheritance chain. If this assertion regresses, the
        // whole crate's Windows TOCTOU model is undermined.
        let root = td();
        let parent = Dir::open_ambient_dir(root.path(), ambient_authority()).unwrap();
        let tmp = TempDir::new_in(&parent, "test-", &DirBuilder::new()).unwrap();

        let owner = super::windows_sec::read_owner_sid(&*tmp).unwrap();
        let ours = super::windows_sec::owner_sid().unwrap();
        assert_eq!(owner.as_slice(), ours);

        assert!(super::windows_sec::dacl_is_protected(&*tmp).unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn windows_is_symlink_refusal_matches_reparse_codes() {
        use windows_sys::Win32::Foundation::{ERROR_CANT_ACCESS_FILE, ERROR_STOPPED_ON_SYMLINK};
        assert!(super::windows_sec::is_symlink_refusal(
            &io::Error::from_raw_os_error(ERROR_STOPPED_ON_SYMLINK as i32)
        ));
        assert!(super::windows_sec::is_symlink_refusal(
            &io::Error::from_raw_os_error(ERROR_CANT_ACCESS_FILE as i32)
        ));
        assert!(!super::windows_sec::is_symlink_refusal(&io::Error::from(
            io::ErrorKind::NotFound
        )));
    }
}
