//! Benchmarks for lock-file serialisation performance and size characteristics.
//!
//! All three size-related benchmarks requested in
//! <https://github.com/conda/rattler/issues/765> are covered:
//!
//! 1. **Raw size** – the number of bytes in the on-disk YAML representation.
//! 2. **Compressed size** – how many bytes remain after zstd compression,
//!    approximating what each blob costs inside a git pack-file.
//! 3. **Git history size** – a temporary git repository is initialised and
//!    the lock file is committed several times to measure exactly how much
//!    each commit adds to the `.git` directory on disk.

use std::hint::black_box;
use std::path::Path;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use rattler_lock::LockFile;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// (display name, path relative to the `test-data/conda-lock/` directory)
const FIXTURES: &[(&str, &str)] = &[
    ("absolute-path", "v4/absolute-path-lock.yml"),
    ("python", "v4/python-lock.yml"),
    ("numpy", "v4/numpy-lock.yml"),
    ("turtlesim", "v4/turtlesim-lock.yml"),
];

fn test_data_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data/conda-lock")
}

fn read_fixture(relative_path: &str) -> Vec<u8> {
    let path = test_data_dir().join(relative_path);
    std::fs::read(&path)
        .unwrap_or_else(|e| panic!("failed to read fixture at {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Size helpers
// ---------------------------------------------------------------------------

/// Compress `data` with zstd at level 3 (git uses zlib at a comparable level).
fn zstd_compress(data: &[u8]) -> Vec<u8> {
    zstd::stream::encode_all(data, 3).expect("zstd compression failed")
}

/// Recursively sum the sizes of all files under `dir`.
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries.flatten().fold(0u64, |acc, entry| {
        let p = entry.path();
        if p.is_dir() {
            acc + dir_size(&p)
        } else {
            acc + p.metadata().map(|m| m.len()).unwrap_or(0)
        }
    })
}

/// Print the raw-vs-compressed size table for all fixtures.
///
/// Written to stderr so it shows up in `cargo bench` output even when the
/// Criterion harness captures stdout.  Returns the raw bytes for each fixture
/// so callers can pass them to `Throughput::Bytes`.
fn print_size_table(fixtures: &[(&str, &str)]) -> Vec<Vec<u8>> {
    eprintln!(
        "\n{:<20} {:>12} {:>14} {:>12}",
        "fixture", "raw (bytes)", "zstd (bytes)", "ratio"
    );
    eprintln!("{:-<60}", "");
    fixtures
        .iter()
        .map(|(name, rel)| {
            let raw = read_fixture(rel);
            let compressed = zstd_compress(&raw);
            let ratio = raw.len() as f64 / compressed.len() as f64;
            eprintln!(
                "{:<20} {:>12} {:>14} {:>11.1}x",
                name,
                raw.len(),
                compressed.len(),
                ratio
            );
            raw
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Benchmark 1: parsing
// ---------------------------------------------------------------------------

/// Measure how fast `LockFile::from_reader` can deserialise each fixture.
///
/// `Throughput::Bytes` is set to the raw file size so Criterion reports MB/s,
/// making it easy to spot regressions in the YAML parser or data-structure
/// construction.
fn bench_parse(c: &mut Criterion) {
    // The size table satisfies issue requirement 1 (raw size) and 2 (compressed
    // size) by printing the numbers before the timed iterations begin.
    let raw_data = print_size_table(FIXTURES);

    let mut group = c.benchmark_group("lock_file/parse");
    group.sample_size(20);

    for ((name, _path), raw) in FIXTURES.iter().zip(&raw_data) {
        group.throughput(Throughput::Bytes(raw.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), raw, |b, raw| {
            b.iter(|| LockFile::from_reader(black_box(raw.as_slice())).unwrap());
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark 2: serialisation
// ---------------------------------------------------------------------------

/// Measure how fast `LockFile::render_to_string` can serialise each fixture.
///
/// `Throughput::Bytes` is set to the rendered YAML length so the report shows
/// how many bytes per second the serialiser produces.
fn bench_render(c: &mut Criterion) {
    let mut group = c.benchmark_group("lock_file/render");
    group.sample_size(20);

    for (name, rel) in FIXTURES {
        let raw = read_fixture(rel);
        let lock_file = LockFile::from_reader(raw.as_slice())
            .unwrap_or_else(|e| panic!("failed to parse fixture '{name}': {e}"));

        // Measure the rendered size once so we can set throughput correctly.
        let rendered_len = lock_file
            .render_to_string()
            .map(|s| s.len() as u64)
            .unwrap_or(raw.len() as u64);

        group.throughput(Throughput::Bytes(rendered_len));
        group.bench_with_input(BenchmarkId::from_parameter(name), &lock_file, |b, lf| {
            b.iter(|| black_box(lf.render_to_string()).unwrap());
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark 3: git repository size with multiple lock-file commits
// ---------------------------------------------------------------------------

/// Measure how much each lock-file commit contributes to a git repository.
///
/// A temporary git repository is initialised and a selection of lock files is
/// committed one after another (simulating a project whose dependencies grow
/// over time).  After each commit `git gc` is run to pack loose objects, and
/// the total `.git` directory size is measured and reported.
///
/// **Serialisation stability note** – the v5 `stability-original` and
/// `stability-shuffled` files contain *identical* packages in different input
/// order.  After a canonical round-trip both render to the same YAML bytes.
/// Committing both therefore adds zero extra bytes to the pack-file, which is
/// reported in the table.  This directly demonstrates that the lock-file
/// serialiser's deterministic ordering keeps git history compact.
///
/// The Criterion group also times the `to_path` (render + write) call that
/// rattler executes every time it decides whether to update the lock file,
/// since that directly affects how quickly the git-tracked file changes.
fn bench_git_history(c: &mut Criterion) {
    // Run git silently; panic if the command is unavailable so the failure is
    // obvious rather than silently skipped.  Returns `true` on success.
    let run_git = |args: &[&str], cwd: &Path| -> bool {
        std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap_or_else(|e| panic!("failed to run `git {args:?}`: {e}"))
            .success()
    };

    let tmp = tempfile::tempdir().expect("creating temp dir for git history measurement");
    let repo = tmp.path();

    assert!(run_git(&["init", "-q"], repo));
    assert!(run_git(
        &["config", "user.email", "bench@example.com"],
        repo
    ));
    assert!(run_git(&["config", "user.name", "Benchmark"], repo));

    let lock_path = repo.join("pixi.lock");

    // Commits to simulate.  The stability pair uses canonical rendering so the
    // bytes actually stored in git match what rattler would write.
    let commits: &[(&str, &str)] = &[
        ("stability-original", "v5/stability-original.yml"),
        (
            "stability-shuffled (re-rendered)",
            "v5/stability-shuffled.yml",
        ),
        ("python (small)", "v4/python-lock.yml"),
        ("numpy (medium)", "v4/numpy-lock.yml"),
    ];

    eprintln!(
        "\n{:<35} {:>16} {:>12}",
        "commit", ".git size (bytes)", "delta (bytes)"
    );
    eprintln!("{:-<67}", "");

    let mut prev_size = 0u64;

    for (label, rel) in commits {
        // For the stability pair, round-trip through the parser so we store the
        // canonical YAML that rattler actually writes, not the raw input.
        let content: Vec<u8> = if rel.starts_with("v5/stability") {
            let raw = read_fixture(rel);
            LockFile::from_reader(raw.as_slice())
                .expect("failed to parse stability fixture")
                .render_to_string()
                .expect("failed to render stability fixture")
                .into_bytes()
        } else {
            read_fixture(rel)
        };

        std::fs::write(&lock_path, &content).expect("writing lock file");
        run_git(&["add", "pixi.lock"], repo);

        // `git commit` exits non-zero when nothing changed.  This is the
        // desired outcome for the stability-shuffled entry: because the
        // canonical render is identical to the previous commit, git refuses to
        // create a new blob, meaning zero extra bytes in the repository.
        let committed = run_git(&["commit", "-q", "-m", &format!("update: {label}")], repo);

        // Pack loose objects to get an accurate, realistic size.
        run_git(&["gc", "--quiet"], repo);

        let git_size = dir_size(&repo.join(".git"));
        let delta = git_size.saturating_sub(prev_size);
        let note = if committed {
            ""
        } else {
            " (no change – stable!)"
        };
        eprintln!("{label:<35} {git_size:>16} {delta:>12}{note}");
        prev_size = git_size;
    }
    eprintln!();

    // Time the render + write cycle: this is the operation that creates or
    // updates the git-tracked lock file, so its speed determines how quickly
    // rattler can decide whether a new commit is needed.
    let numpy_lock = LockFile::from_reader(read_fixture("v4/numpy-lock.yml").as_slice()).unwrap();
    let tmp2 = tempfile::tempdir().unwrap();
    let out_path = tmp2.path().join("pixi.lock");

    let rendered_len = numpy_lock
        .render_to_string()
        .map(|s| s.len() as u64)
        .unwrap();

    let mut group = c.benchmark_group("lock_file/git_history");
    group.sample_size(20);
    group.throughput(Throughput::Bytes(rendered_len));
    group.bench_function("render-and-write/numpy", |b| {
        b.iter(|| numpy_lock.to_path(black_box(&out_path)).unwrap());
    });
    group.finish();
}

// ---------------------------------------------------------------------------
// Bonus: serialisation stability (canonical ordering)
// ---------------------------------------------------------------------------

/// Verify that round-tripping a lock file through parse → render always
/// produces the same bytes regardless of the input ordering.
///
/// This is the property that makes `bench_git_history` produce small deltas:
/// if re-rendering a lock file with the same packages always gives the same
/// YAML, git never needs to store a new blob for a no-op re-solve.
fn bench_stability(c: &mut Criterion) {
    let original_raw = read_fixture("v5/stability-original.yml");
    let shuffled_raw = read_fixture("v5/stability-shuffled.yml");

    let original = LockFile::from_reader(original_raw.as_slice())
        .expect("failed to parse stability-original.yml");
    let shuffled = LockFile::from_reader(shuffled_raw.as_slice())
        .expect("failed to parse stability-shuffled.yml");

    let rendered_original = original
        .render_to_string()
        .expect("failed to render original");
    let rendered_shuffled = shuffled
        .render_to_string()
        .expect("failed to render shuffled");

    // Correctness assertion: both round-trips must yield the same bytes.
    assert_eq!(
        rendered_original, rendered_shuffled,
        "serialisation is not stable: two orderings of the same packages \
         produce different YAML, which causes spurious git history growth"
    );

    let compressed = zstd_compress(rendered_original.as_bytes());
    eprintln!(
        "\nstability pair – rendered: {} bytes, zstd: {} bytes",
        rendered_original.len(),
        compressed.len(),
    );

    let mut group = c.benchmark_group("lock_file/stability");
    group.sample_size(20);
    group.throughput(Throughput::Bytes(rendered_original.len() as u64));

    group.bench_function("render-original", |b| {
        b.iter(|| black_box(original.render_to_string()).unwrap());
    });
    group.bench_function("render-shuffled", |b| {
        b.iter(|| black_box(shuffled.render_to_string()).unwrap());
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    bench_parse,
    bench_render,
    bench_git_history,
    bench_stability
);
criterion_main!(benches);
