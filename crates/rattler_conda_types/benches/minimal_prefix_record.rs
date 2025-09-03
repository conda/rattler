use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use rattler_conda_types::{MinimalPrefixRecord, PrefixRecord};
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};

fn get_test_files() -> Vec<(PathBuf, usize)> {
    let conda_meta_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("test-data")
        .join("conda-meta");

    let mut files: Vec<(PathBuf, usize)> = fs::read_dir(&conda_meta_path)
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension()?.to_str()? == "json" {
                let size = entry.metadata().ok()?.len() as usize;
                Some((path, size))
            } else {
                None
            }
        })
        .collect();

    // Sort by size to have consistent ordering
    files.sort_by_key(|(_, size)| *size);
    files
}

// Helper function to convert PrefixRecord JSON to new format
fn convert_to_new_format(
    original_path: &Path,
    new_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let record = PrefixRecord::from_path(original_path)?;
    record.write_to_path(new_path, true)?;
    Ok(())
}

// Benchmark 3: MinimalPrefixRecord parsing real conda-meta files (old format)
fn bench_minimal_prefix_record_old_and_new_format_on_conda_meta(c: &mut Criterion) {
    let files = get_test_files();
    let total_size: usize = files.iter().map(|(_, size)| size).sum();

    let temp_dir = tempfile::tempdir().unwrap();

    // Copy original conda-meta files to temp directory for old format testing
    let old_conda_meta = temp_dir.path().join("conda-meta-old");
    fs::create_dir_all(&old_conda_meta).unwrap();

    for (original_path, _) in &files {
        let file_name = original_path.file_name().unwrap();
        let dest_path = old_conda_meta.join(file_name);
        fs::copy(original_path, dest_path).unwrap();
    }

    let mut group = c.benchmark_group("PrefixRecord ordering");
    // Might slightly change between old and new format, but difference is negligible.
    group.throughput(Throughput::Bytes(total_size as u64));

    group.bench_function("MinimalPrefixRecord original conda meta", |b| {
        b.iter(|| {
            for (path, _) in &files {
                let _ = black_box(MinimalPrefixRecord::from_path(path));
            }
        });
    });

    let new_conda_meta = temp_dir.path().join("conda-meta-new");
    fs::create_dir_all(&new_conda_meta).unwrap();

    for (original_path, _) in &files {
        let file_name = original_path.file_name().unwrap();
        let new_path = new_conda_meta.join(file_name);
        convert_to_new_format(original_path, &new_path).unwrap();
    }

    group.bench_function("MinimalPrefixRecord new conda meta", |b| {
        b.iter(|| {
            for (original_path, _) in &files {
                let file_name = original_path.file_name().unwrap();
                let new_path = new_conda_meta.join(file_name);
                let _ = black_box(MinimalPrefixRecord::from_path(&new_path));
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_minimal_prefix_record_old_and_new_format_on_conda_meta,
);
criterion_main!(benches);
