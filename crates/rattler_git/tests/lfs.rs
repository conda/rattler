//! Integration tests for the Git LFS fetch path. Builds a tiny fixture repo
//! with `*.bin filter=lfs` in `.gitattributes` and one binary file.
//! Requires `git-lfs` on the host; tests skip themselves when it's missing.

use std::path::Path;
use std::process::Command;

use rattler_git::LazyClient;
use rattler_git::{GitUrl, sha::GitSha, source::GitSource};
use reqwest_middleware::ClientWithMiddleware;
use url::Url;

/// `LazyClient` that panics if HTTP is touched. file:// URLs never trigger it.
fn panic_client() -> LazyClient {
    LazyClient::new(|| -> ClientWithMiddleware {
        panic!("network should not be used in LFS tests")
    })
}

/// Returns whether `path` looks like a git-lfs pointer file.
fn is_lfs_pointer(path: &Path) -> bool {
    let contents = fs_err::read_to_string(path).unwrap_or_default();
    contents.starts_with("version https://git-lfs.github.com/spec/")
}

/// Skip the test when `git lfs version` doesn't work on this host.
fn require_git_lfs(test: &str) -> bool {
    let ok = Command::new("git")
        .args(["lfs", "version"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipping {test}: git-lfs is not installed");
    }
    ok
}

/// A tiny git repository with one LFS-tracked file (`data.bin`).
struct LfsFixture {
    /// Kept alive to prevent cleanup until the fixture is dropped.
    _tempdir: tempfile::TempDir,
    repo_path: std::path::PathBuf,
    base_url: Url,
    head: String,
}

impl LfsFixture {
    fn new() -> Self {
        let tempdir = tempfile::tempdir().expect("failed to create temp dir");
        let repo_path = tempdir.path().join("lfs-sample");
        fs_err::create_dir_all(&repo_path).unwrap();

        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&repo_path)
                .output()
                .unwrap_or_else(|err| panic!("failed to spawn `git {}`: {err}", args.join(" ")));
            assert!(
                output.status.success(),
                "`git {}` failed: stdout={:?} stderr={:?}",
                args.join(" "),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };

        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "test@test.com"]);
        git(&["config", "user.name", "Test"]);
        // Disable signing so a global `commit.gpgSign = true` can't interfere.
        git(&["config", "commit.gpgsign", "false"]);
        git(&["lfs", "install", "--local"]);

        fs_err::write(
            repo_path.join(".gitattributes"),
            "*.bin filter=lfs diff=lfs merge=lfs -text\n",
        )
        .unwrap();
        fs_err::write(repo_path.join("README.md"), "# lfs sample\n").unwrap();
        fs_err::write(
            repo_path.join("data.bin"),
            b"\x00\x01\x02\x03binary payload\xff\xfe",
        )
        .unwrap();

        git(&["add", "."]);
        git(&["commit", "--message", "v0.1.0"]);
        let head = git(&["rev-parse", "HEAD"]);

        // Sanity: the committed data.bin must be an LFS pointer.
        let pointer = git(&["show", "HEAD:data.bin"]);
        assert!(
            pointer.starts_with("version https://git-lfs.github.com/spec/"),
            "HEAD:data.bin should be an LFS pointer, got: {pointer:?}"
        );

        let base_url = Url::from_directory_path(&repo_path).unwrap();
        Self {
            _tempdir: tempdir,
            repo_path,
            base_url,
            head,
        }
    }
}

/// The LFS fixture itself builds: `data.bin` lands in the repo as an LFS
/// pointer with the blob present under `.git/lfs/objects/`.
#[test]
fn fixture_builds_with_lfs() {
    if !require_git_lfs("fixture_builds_with_lfs") {
        return;
    }
    let repo = LfsFixture::new();

    // The actual blob is in the repo's LFS object store.
    let objects = repo.repo_path.join(".git/lfs/objects");
    assert!(
        objects.is_dir() && fs_err::read_dir(&objects).unwrap().next().is_some(),
        "expected LFS objects under {}",
        objects.display()
    );
}

/// With `lfs == Some(false)`, `GitSource::fetch` force-skips the smudge
/// filter and leaves LFS pointers in the checkout.
#[test]
fn fetch_without_lfs_leaves_pointer() {
    if !require_git_lfs("fetch_without_lfs_leaves_pointer") {
        return;
    }
    let repo = LfsFixture::new();
    let cache = tempfile::tempdir().unwrap();

    let git_url = GitUrl::try_from(repo.base_url.clone()).unwrap();
    let fetch = GitSource::new(git_url, panic_client(), cache.path())
        .with_lfs(Some(false))
        .fetch()
        .expect("fetch should succeed");

    assert!(!fetch.lfs_ready(), "LFS was not requested");
    let data = fetch.path().join("data.bin");
    assert!(data.is_file(), "data.bin missing from checkout");
    assert!(
        is_lfs_pointer(&data),
        "data.bin should still be a pointer when LFS is disabled"
    );
}

/// With `lfs == Some(true)`, `GitSource::fetch` runs `git lfs fetch`,
/// validates with `git lfs fsck`, and materialises pointer files into the
/// real blob content during the subsequent `git reset --hard`.
#[test]
fn fetch_with_lfs_materialises_blob() {
    if !require_git_lfs("fetch_with_lfs_materialises_blob") {
        return;
    }
    let repo = LfsFixture::new();
    let original = fs_err::read(repo.repo_path.join("data.bin")).unwrap();
    let cache = tempfile::tempdir().unwrap();

    let git_url = GitUrl::try_from(repo.base_url.clone()).unwrap();
    let fetch = GitSource::new(git_url, panic_client(), cache.path())
        .with_lfs(Some(true))
        .fetch()
        .expect("fetch should succeed");

    assert!(
        fetch.lfs_ready(),
        "fsck should pass for a healthy LFS fixture"
    );

    let data = fetch.path().join("data.bin");
    assert!(data.is_file());
    assert!(
        !is_lfs_pointer(&data),
        "data.bin should be the real blob, not a pointer"
    );
    let got = fs_err::read(&data).unwrap();
    assert_eq!(
        got, original,
        "checked-out data.bin should match fixture source"
    );
}

/// Second fetch against a warm cache (same `cache` dir, same precise rev)
/// hits the cached-DB branch in `GitSource::fetch`. With LFS requested, the
/// branch also requires `db.contains_lfs_artifacts(rev)`; it does after the
/// first fetch populated `.git/lfs/objects/`, so the second fetch returns
/// `lfs_ready == true` without touching the remote.
#[test]
fn cached_fetch_with_lfs_artifacts_is_ready() {
    if !require_git_lfs("cached_fetch_with_lfs_artifacts_is_ready") {
        return;
    }
    let repo = LfsFixture::new();
    let cache = tempfile::tempdir().unwrap();
    let head: GitSha = repo.head.parse().unwrap();

    let make_source = || {
        let url = GitUrl::try_from(repo.base_url.clone())
            .unwrap()
            .with_precise(head);
        GitSource::new(url, panic_client(), cache.path()).with_lfs(Some(true))
    };

    // Warm the cache.
    let first = make_source().fetch().expect("first fetch should succeed");
    assert!(first.lfs_ready());

    // Second fetch should reuse the DB and skip the network entirely while
    // still reporting LFS as ready.
    let second = make_source().fetch().expect("cached fetch should succeed");
    assert!(second.lfs_ready());
    assert_eq!(second.commit(), first.commit());
}
