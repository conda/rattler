use std::{
    path::{Path, PathBuf},
    process::Command,
};

use rattler_git::{GitError, GitUrl, git::GitReference, source::GitSource};
use rattler_networking::LazyClient;
use reqwest_middleware::ClientWithMiddleware;

fn panic_client() -> LazyClient {
    LazyClient::new(|| -> ClientWithMiddleware { panic!("local repositories must not use HTTP") })
}

/// Runs git, returning `(succeeded, stdout, stderr)`.
fn git_raw(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
        String::from_utf8_lossy(&output.stderr).trim().to_string(),
    )
}

fn git(repo: &Path, args: &[&str]) -> String {
    let (succeeded, stdout, stderr) = git_raw(repo, args);
    assert!(succeeded, "git {args:?} in {}: {stderr}", repo.display());
    stdout
}

fn init_repo(dir: &tempfile::TempDir) -> PathBuf {
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    // Commits and tags must not depend on the developer's signing setup.
    git(&repo, &["config", "commit.gpgSign", "false"]);
    git(&repo, &["config", "tag.gpgSign", "false"]);
    git(&repo, &["config", "user.name", "Test"]);
    repo
}

fn fixture() -> (tempfile::TempDir, url::Url, String) {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(&dir);

    std::fs::write(repo.join("file.txt"), "content").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "initial"]);
    git(&repo, &["tag", "v1.0.0"]);
    let commit = git(&repo, &["rev-parse", "HEAD"]);
    let url = url::Url::from_directory_path(repo).unwrap();
    (dir, url, commit)
}

/// A repository whose `v1.0.0` tag sits on top of an ancestor carrying its own
/// `v0.9.0` tag, plus an `unrelated` tag on a history of its own. Checking out
/// `v1.0.0` therefore depends on refs the request never names.
struct History {
    _dir: tempfile::TempDir,
    url: url::Url,
    head: String,
    ancestor: String,
    unrelated: String,
}

impl History {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo(&dir);

        std::fs::write(repo.join("file.txt"), "first").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "first"]);
        git(&repo, &["tag", "v0.9.0"]);
        let ancestor = git(&repo, &["rev-parse", "HEAD"]);

        std::fs::write(repo.join("file.txt"), "second").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "second"]);
        git(&repo, &["tag", "v1.0.0"]);
        let head = git(&repo, &["rev-parse", "HEAD"]);

        git(&repo, &["checkout", "--orphan", "side", "--quiet"]);
        std::fs::write(repo.join("other.txt"), "unrelated").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "unrelated"]);
        git(&repo, &["tag", "unrelated"]);
        let unrelated = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["checkout", "v1.0.0", "--quiet"]);

        Self {
            url: url::Url::from_directory_path(&repo).unwrap(),
            _dir: dir,
            head,
            ancestor,
            unrelated,
        }
    }

    fn source(&self, rev: &str, cache: &Path) -> GitSource {
        let git = GitUrl::from_reference(self.url.clone(), GitReference::from_rev(rev.to_string()));
        GitSource::new(git, panic_client(), cache)
    }

    /// Removes the loose object `oid` from the database at `db`.
    fn strip_object(db: &Path, oid: &str) {
        let object = db.join(".git/objects").join(&oid[..2]).join(&oid[2..]);
        assert!(object.is_file(), "{} is a loose object", object.display());
        std::fs::remove_file(&object).unwrap();
    }
}

/// The single git database a cache holds after fetching one repository.
fn cache_database(cache: &Path) -> PathBuf {
    let mut databases = std::fs::read_dir(cache.join("db"))
        .unwrap()
        .map(|entry| entry.unwrap().path());
    let database = databases.next().expect("the cache holds a database");
    assert!(databases.next().is_none(), "the cache holds one database");
    database
}

#[test]
fn rev_naming_a_tag_resolves_to_the_tag_commit() {
    let (_fixture, repository, commit) = fixture();
    let cache = tempfile::tempdir().unwrap();
    let git = GitUrl::from_reference(repository, GitReference::from_rev("v1.0.0".to_string()));

    let fetch = GitSource::new(git, panic_client(), cache.path())
        .fetch()
        .unwrap();

    assert_eq!(fetch.commit().to_string(), commit);
    assert!(fetch.path().join("file.txt").is_file());
}

#[test]
fn unresolved_rev_names_the_reference() {
    let (_fixture, repository, _) = fixture();
    let cache = tempfile::tempdir().unwrap();
    let git = GitUrl::from_reference(repository, GitReference::from_rev("deadbeef".to_string()));

    let err = GitSource::new(git, panic_client(), cache.path())
        .fetch()
        .expect_err("missing revision must fail");

    assert!(matches!(err, GitError::ReferenceNotFound { .. }));
    assert!(err.to_string().contains("deadbeef"));
}

/// A database that lost an object one of its refs points at cannot be cloned
/// from, and a fetch into it does not restore the object.
#[test]
fn checkout_rebuilds_a_database_missing_objects() {
    let history = History::new();
    let cache = tempfile::tempdir().unwrap();

    history.source("v1.0.0", cache.path()).fetch().unwrap();

    // Drop the object behind a tag the request does not name.
    let database = cache_database(cache.path());
    assert_eq!(
        git(&database, &["rev-parse", "refs/tags/v0.9.0"]),
        history.ancestor
    );
    History::strip_object(&database, &history.ancestor);
    std::fs::remove_dir_all(cache.path().join("checkouts")).unwrap();

    let fetch = history.source("v1.0.0", cache.path()).fetch().unwrap();

    assert_eq!(fetch.commit().to_string(), history.head);
    assert_eq!(
        std::fs::read_to_string(fetch.path().join("file.txt")).unwrap(),
        "second"
    );
}

/// The same damage, hidden from the fetch by a commit-graph that still vouches
/// for the missing object: git transfers nothing and reports success, so only
/// the local clone notices. This is the shape the failure takes against a
/// remote that has nothing new to send.
#[test]
fn checkout_rebuilds_a_database_its_commit_graph_vouches_for() {
    let history = History::new();
    let cache = tempfile::tempdir().unwrap();

    history.source("v1.0.0", cache.path()).fetch().unwrap();
    history.source("unrelated", cache.path()).fetch().unwrap();

    let database = cache_database(cache.path());
    git(&database, &["commit-graph", "write", "--reachable"]);
    History::strip_object(&database, &history.unrelated);
    std::fs::remove_dir_all(cache.path().join("checkouts")).unwrap();

    // The premise: git is fooled, the clone is not.
    let (fetched, _, _) = git_raw(
        &database,
        &[
            "fetch",
            "--force",
            history.url.as_str(),
            "+refs/tags/v1.0.0:refs/remotes/origin/tags/v1.0.0",
        ],
    );
    let probe = cache.path().join("probe");
    let (cloned, _, _) = git_raw(
        cache.path(),
        &[
            "clone",
            "--local",
            &database.to_string_lossy(),
            &probe.to_string_lossy(),
        ],
    );
    assert!(fetched, "the fetch was supposed to succeed");
    assert!(!cloned, "the clone was supposed to fail");
    std::fs::remove_dir_all(&probe).ok();

    let fetch = history.source("v1.0.0", cache.path()).fetch().unwrap();

    assert_eq!(fetch.commit().to_string(), history.head);
}

/// A database git only dislikes for a reason that does not stop a clone — a
/// damaged commit-graph is what an interrupted repack leaves behind — must
/// survive a checkout failure that has nothing to do with it.
#[test]
fn unrelated_checkout_failure_keeps_a_usable_database() {
    let history = History::new();
    let cache = tempfile::tempdir().unwrap();

    history.source("v1.0.0", cache.path()).fetch().unwrap();
    let database = cache_database(cache.path());
    git(&database, &["commit-graph", "write", "--reachable"]);
    let graph = database.join(".git/objects/info/commit-graph");
    assert!(graph.is_file(), "no commit-graph was written");
    std::fs::remove_file(&graph).unwrap();
    std::fs::write(&graph, vec![0u8; 512]).unwrap();

    let marker = database.join(".git/marker");
    std::fs::write(&marker, "original database").unwrap();

    // Occupy the directory the checkout wants with a file.
    let checkouts = cache.path().join("checkouts");
    std::fs::remove_dir_all(&checkouts).unwrap();
    std::fs::create_dir(&checkouts).unwrap();
    std::fs::write(checkouts.join(database.file_name().unwrap()), "").unwrap();

    history
        .source("v1.0.0", cache.path())
        .fetch()
        .expect_err("the checkout cannot succeed");

    assert!(marker.is_file(), "a usable database was thrown away");
}

/// A rebuild that cannot reach the remote leaves the cache as it was.
#[test]
fn failed_rebuild_keeps_the_existing_database() {
    let history = History::new();
    let cache = tempfile::tempdir().unwrap();

    history.source("v1.0.0", cache.path()).fetch().unwrap();

    let database = cache_database(cache.path());
    History::strip_object(&database, &history.ancestor);
    std::fs::remove_dir_all(cache.path().join("checkouts")).unwrap();
    std::fs::remove_dir_all(history.url.to_file_path().unwrap()).unwrap();

    history
        .source("v1.0.0", cache.path())
        .fetch()
        .expect_err("an unreachable remote must fail");

    assert_eq!(
        git(&database, &["rev-parse", "refs/tags/v1.0.0"]),
        history.head
    );
    let leftovers = std::fs::read_dir(database.parent().unwrap())
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .is_ok_and(|entry| entry.file_name().to_string_lossy().starts_with(".rebuild-"))
        })
        .count();
    assert_eq!(leftovers, 0, "a failed rebuild leaves no staging directory");
}
