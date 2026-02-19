use criterion::{criterion_group, criterion_main, Criterion};
use flate2::{write::GzEncoder, Compression};
use std::fs;
use std::io::Write;
use std::process::Command;
use tempfile::TempDir;

const LOCK_PATH: &str = "../../test-data/conda-lock/v4/turtlesim-lock.yml";

fn bench_lock_file_size(c: &mut Criterion) {
    c.bench_function("lock_file_size", |b| {
        b.iter(|| {
            let metadata = fs::metadata(LOCK_PATH).expect("lock file not found");
            metadata.len()
        })
    });
}

fn bench_compressed_lock_file_size(c: &mut Criterion) {
    c.bench_function("compressed_lock_file_size", |b| {
        b.iter(|| {
            let data = fs::read(LOCK_PATH).expect("lock file not found");
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&data).unwrap();
            let compressed = encoder.finish().unwrap();
            compressed.len()
        })
    });
}

fn bench_git_repo_size(c: &mut Criterion) {
    c.bench_function("git_repo_size_with_lock_changes", |b| {
        b.iter(|| {
            let temp_dir = TempDir::new().unwrap();
            let repo_path = temp_dir.path();

            // Init git repo
            Command::new("git")
                .args(["init"])
                .current_dir(repo_path)
                .output()
                .unwrap();

            let lock_content = fs::read(LOCK_PATH).unwrap();

            for i in 0..5 {
                let file_path = repo_path.join("lock.yml");
                fs::write(&file_path, &lock_content).unwrap();

                Command::new("git")
                    .args(["add", "."])
                    .current_dir(repo_path)
                    .output()
                    .unwrap();

                Command::new("git")
                    .args(["commit", "-m", &format!("commit {}", i)])
                    .env("GIT_AUTHOR_NAME", "bench")
                    .env("GIT_AUTHOR_EMAIL", "bench@example.com")
                    .env("GIT_COMMITTER_NAME", "bench")
                    .env("GIT_COMMITTER_EMAIL", "bench@example.com")
                    .current_dir(repo_path)
                    .output()
                    .unwrap();
            }

            // Measure .git folder size
            let git_dir = repo_path.join(".git");
            dir_size(&git_dir)
        })
    });
}

fn dir_size(path: &std::path::Path) -> u64 {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                dir_size(&entry.path())
            } else {
                metadata.len()
            }
        })
        .sum()
}

criterion_group!(
    benches,
    bench_lock_file_size,
    bench_compressed_lock_file_size,
    bench_git_repo_size
);
criterion_main!(benches);

