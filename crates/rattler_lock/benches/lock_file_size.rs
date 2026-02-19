use std::fs;
use std::io::Write;
use std::process::Command;
use tempfile::TempDir;
use flate2::{write::GzEncoder, Compression};

const LOCK_PATH: &str = "../../test-data/conda-lock/v4/turtlesim-lock.yml";

#[test]
fn report_lock_file_size() {
    let metadata = fs::metadata(LOCK_PATH)
        .expect("lock file not found");

    println!("Lock file size: {} bytes", metadata.len());
}

#[test]
fn report_compressed_lock_file_size() {
    let data = fs::read(LOCK_PATH)
        .expect("lock file not found");

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&data).unwrap();
    let compressed = encoder.finish().unwrap();

    println!("Compressed lock file size: {} bytes", compressed.len());
}

#[test]
fn report_git_repo_size_with_lock_changes() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    Command::new("git")
        .args(["init"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    for i in 0..5 {
        let content = format!("change {i}");
        let file_path = repo_path.join("lock.yml");
        fs::write(&file_path, content).unwrap();

        Command::new("git")
            .args(["add", "."])
            .current_dir(repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", &format!("commit {i}")])
            .current_dir(repo_path)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();
    }

    let git_dir = repo_path.join(".git");
    let size = dir_size(&git_dir);

    println!("Git repo size after lock changes: {} bytes", size);
}

fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0;
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        let metadata = entry.metadata().unwrap();
        if metadata.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += metadata.len();
        }
    }
    total
}
