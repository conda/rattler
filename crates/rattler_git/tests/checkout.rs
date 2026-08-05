use std::process::Command;

use rattler_git::{GitError, GitUrl, git::GitReference, source::GitSource};
use rattler_networking::LazyClient;
use reqwest_middleware::ClientWithMiddleware;

fn panic_client() -> LazyClient {
    LazyClient::new(|| -> ClientWithMiddleware { panic!("local repositories must not use HTTP") })
}

fn fixture() -> (tempfile::TempDir, url::Url, String) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    };

    std::fs::create_dir(&repo).unwrap();
    git(&["init"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    std::fs::write(repo.join("file.txt"), "content").unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "initial"]);
    git(&["tag", "v1.0.0"]);
    let commit = git(&["rev-parse", "HEAD"]);
    let url = url::Url::from_directory_path(repo).unwrap();
    (dir, url, commit)
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
