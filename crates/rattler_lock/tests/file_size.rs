//! Deterministic size checks for lock files.
//!
//! These tests track the raw and compressed sizes of lock file fixtures so that
//! changes to the serializer that affect on-disk size show up as snapshot diffs
//! in pull requests. This covers the three metrics from
//! <https://github.com/conda/rattler/issues/765>:
//!
//! 1. Lock file size (raw rendered bytes)
//! 2. Compressed lock file size (zstd, approximating git pack-file cost)
//! 3. Git repo size with multiple commits (temp repo with `git gc`)

use std::path::Path;

use rattler_lock::LockFile;

fn test_data_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data/conda-lock")
}

fn read_fixture(rel: &str) -> Vec<u8> {
    let path = test_data_dir().join(rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Round-trip a fixture through parse → render and return the rendered bytes.
fn rendered(rel: &str) -> String {
    let raw = read_fixture(rel);
    LockFile::from_reader(raw.as_slice())
        .unwrap_or_else(|e| panic!("parse {rel}: {e}"))
        .render_to_string()
        .unwrap_or_else(|e| panic!("render {rel}: {e}"))
}

fn zstd_compress(data: &[u8]) -> Vec<u8> {
    zstd::stream::encode_all(data, 3).expect("zstd compression failed")
}

// -----------------------------------------------------------------------
// 1 & 2: raw and compressed sizes
// -----------------------------------------------------------------------

#[test]
fn lock_file_sizes() {
    let fixtures = [
        ("python", "v4/python-lock.yml"),
        ("numpy", "v4/numpy-lock.yml"),
        ("turtlesim", "v4/turtlesim-lock.yml"),
    ];

    let mut report = String::new();
    for (name, rel) in &fixtures {
        let yaml = rendered(rel);
        let compressed = zstd_compress(yaml.as_bytes());
        report.push_str(&format!(
            "{name}: raw={}, zstd={}, ratio={:.1}x\n",
            yaml.len(),
            compressed.len(),
            yaml.len() as f64 / compressed.len() as f64,
        ));
    }

    insta::assert_snapshot!(report);
}

// -----------------------------------------------------------------------
// 3: git repo size across multiple commits
// -----------------------------------------------------------------------

/// Run a git command silently. Returns true on success.
fn git(args: &[&str], cwd: &Path) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"))
        .success()
}

/// Recursively sum file sizes under `dir`.
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries.flatten().fold(0u64, |acc, e| {
        let p = e.path();
        if p.is_dir() {
            acc + dir_size(&p)
        } else {
            acc + p.metadata().map(|m| m.len()).unwrap_or(0)
        }
    })
}

#[test]
fn git_history_size() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();

    assert!(git(&["init", "-q"], repo));
    assert!(git(&["config", "user.email", "test@test.com"], repo));
    assert!(git(&["config", "user.name", "Test"], repo));

    let lock_path = repo.join("pixi.lock");

    // Commit a sequence of lock files and record the .git size after each.
    // The stability pair is round-tripped so we store canonical YAML.
    let commits: &[(&str, &str)] = &[
        ("stability-original", "v5/stability-original.yml"),
        ("stability-shuffled", "v5/stability-shuffled.yml"),
        ("python", "v4/python-lock.yml"),
        ("numpy", "v4/numpy-lock.yml"),
    ];

    let mut sizes: Vec<(&str, u64, bool)> = Vec::new();

    for (label, rel) in commits {
        let content = if rel.starts_with("v5/stability") {
            rendered(rel).into_bytes()
        } else {
            read_fixture(rel)
        };

        std::fs::write(&lock_path, &content).unwrap();
        git(&["add", "pixi.lock"], repo);
        let committed = git(&["commit", "-q", "-m", label], repo);
        git(&["gc", "--quiet"], repo);

        let size = dir_size(&repo.join(".git"));
        sizes.push((label, size, committed));
    }

    // The stability-shuffled commit must produce no new blob because the
    // canonical render is byte-for-byte identical to the original.
    let (_, _, shuffled_committed) = sizes[1];
    assert!(
        !shuffled_committed,
        "stability-shuffled should have been a no-op commit \
         (serialization is not deterministic!)"
    );

    // Print the table for human inspection during `cargo test -- --nocapture`.
    eprintln!("\n{:<25} {:>12} {:>12}", "commit", ".git (bytes)", "delta");
    eprintln!("{:-<52}", "");
    let mut prev = 0u64;
    for (label, size, committed) in &sizes {
        let delta = size.saturating_sub(prev);
        let note = if *committed { "" } else { " (no-op)" };
        eprintln!("{label:<25} {size:>12} {delta:>12}{note}");
        prev = *size;
    }
    eprintln!();
}

// -----------------------------------------------------------------------
// Bonus: serialization stability
// -----------------------------------------------------------------------

#[test]
fn serialization_is_stable() {
    let a = rendered("v5/stability-original.yml");
    let b = rendered("v5/stability-shuffled.yml");
    assert_eq!(
        a, b,
        "different input orderings must produce identical output"
    );
}
