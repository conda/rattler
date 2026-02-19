use criterion::{criterion_group, criterion_main, Criterion};
use flate2::{write::GzEncoder, Compression};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const LOCK_PATH: &str = "../../test-data/conda-lock/v4/turtlesim-lock.yml";

fn dir_size(path: &Path) -> u64 {
    fs::read_dir(path)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| {
            let path = entry.path();
            if path.is_file() {
                fs::metadata(path).unwrap().len()
            } else {
                dir_size(&path)
            }
        })
        .sum()
}

fn bench_lock_file_size(c: &mut Criterion) {
    c.bench_function("lock_file_size", |b| {
        b.iter(|| {
            let metadata = fs::metadata(LOCK_PATH).expect("lock file not found");
            metadata.len()
        });
    });

    c.bench_function("compressed_lock_file_size", |b| {
        b.iter(|| {
            let data = fs::read(LOCK_PATH).expect("lock file not found");

            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());

            encoder.write_all(&data).unwrap();
            let compressed = encoder.finish().unwrap();

            compressed.len()
        });
    });

    c.bench_function("git_repo_size_with_lock_changes", |b| {
        b.iter(|| {
            let temp_dir = TempDir::new().unwrap();
            let repo_path = temp_dir.path();

            Command::new("git")
                .args(["init"])
                .current_dir(repo_path)
                .output()
                .unwrap();

            fs::copy(LOCK_PATH, repo_path.join("conda-lock.yml")).unwrap();

            for i in 0..5 {
                Command::new("git")
                    .args(["add", "."])
                    .current_dir(repo_path)
                    .output()
                    .unwrap();

                Command::new("git")
                    .args(["commit", "-m", &format!("commit {i}")])
                    .current_dir(repo_path)
                    .env("GIT_AUTHOR_NAME", "test")
                    .env("GIT_AUTHOR_EMAIL", "test@example.com")
                    .env("GIT_COMMITTER_NAME", "test")
                    .env("GIT_COMMITTER_EMAIL", "test@example.com")
                    .output()
                    .unwrap();
            }

            let git_dir = repo_path.join(".git");
            dir_size(&git_dir)
        });
    });
}

criterion_group!(benches, bench_lock_file_size);
criterion_main!(benches);
