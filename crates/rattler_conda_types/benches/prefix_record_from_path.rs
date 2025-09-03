use criterion::{criterion_group, criterion_main, Criterion};
use rattler_conda_types::RecordFromPath;
use rattler_conda_types::{
    MinimalPrefixCollection, MinimalPrefixRecord, PackageRecord, PrefixRecord,
};
use std::hint::black_box;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

fn process_json_files_from_dir(dir: &Path) {
    let entries = fs::read_dir(dir).expect("Directory not found");

    for entry in entries {
        let entry = entry.expect("Unable to read entry");
        let path = entry.path();

        black_box(PrefixRecord::from_path(path).unwrap());
    }
}

fn load_as_prefix_record(dir: &Path) -> Vec<PrefixRecord> {
    black_box(PrefixRecord::collect_from_prefix::<PrefixRecord>(dir).unwrap())
}

fn load_as_package_record(dir: &Path) -> Vec<PackageRecord> {
    black_box(PrefixRecord::collect_from_prefix::<PackageRecord>(dir).unwrap())
}

fn process_minimal_json_files_from_dir(dir: &Path) {
    let entries = fs::read_dir(dir).expect("Directory not found");

    for entry in entries {
        let entry = entry.expect("Unable to read entry");
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            black_box(MinimalPrefixRecord::from_path(&path).unwrap());
        }
    }
}
fn process_package_record_files_from_dir(dir: &Path) {
    let entries = fs::read_dir(dir).expect("Directory not found");

    for entry in entries {
        let entry = entry.expect("Unable to read entry");
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            black_box(PackageRecord::from_path(&path).unwrap());
        }
    }
}

fn load_as_minimal_prefix_record(dir: &Path) -> Vec<PrefixRecord> {
    black_box(PrefixRecord::collect_minimal_from_prefix(dir).unwrap())
}

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

fn criterion_benchmark(c: &mut Criterion) {
    // Create two directoriew with old and new format.
    let files = get_test_files();

    let temp_dir = tempfile::tempdir().unwrap();

    // Copy original conda-meta files to temp directory for old format testing
    let old_conda_meta_root = temp_dir.path().join("conda-meta-old");
    let old_conda_meta = old_conda_meta_root.join("conda-meta");
    fs::create_dir_all(&old_conda_meta).unwrap();

    for (original_path, _) in &files {
        let file_name = original_path.file_name().unwrap();
        let dest_path = old_conda_meta.join(file_name);
        fs::copy(original_path, dest_path).unwrap();
    }

    // Create new format files using convert_to_new_format
    let new_conda_meta_root = temp_dir.path().join("conda-meta-new");
    let new_conda_meta = new_conda_meta_root.join("conda-meta");
    fs::create_dir_all(&new_conda_meta).unwrap();

    for (original_path, _) in &files {
        let file_name = original_path.file_name().unwrap();
        let new_path = new_conda_meta.join(file_name);
        convert_to_new_format(original_path, &new_path).unwrap();
    }

    let old_test_file = old_conda_meta.join("rust-1.88.0-h1a8d7c4_0.json");

    let mut super_long_file: PrefixRecord = PrefixRecord::from_path(&old_test_file).unwrap();
    // duplicate data until we have 20k paths
    let files = super_long_file.files.clone();
    while super_long_file.files.len() < 20_000 {
        super_long_file.files.extend(files.clone());
    }

    let filename = format!(
        "{}-{}-{}.json",
        super_long_file
            .repodata_record
            .package_record
            .name
            .as_normalized(),
        super_long_file.repodata_record.package_record.version,
        super_long_file.repodata_record.package_record.build
    );
    let new_long_file_path = temp_dir.path().join(filename);
    serde_json::to_writer(File::create(&new_long_file_path).unwrap(), &super_long_file).unwrap();

    // Serial Processing Benchmarks (one file at a time)
    let mut serial_group = c.benchmark_group("Serial Processing");
    serial_group.bench_function("PrefixRecord", |b| {
        b.iter(|| process_json_files_from_dir(&old_conda_meta));
    });
    serial_group.bench_function("PackageRecord", |b| {
        b.iter(|| process_package_record_files_from_dir(&old_conda_meta));
    });
    serial_group.bench_function("MinimalPrefixRecord", |b| {
        b.iter(|| process_minimal_json_files_from_dir(&old_conda_meta));
    });
    serial_group.bench_function("MinimalPrefixRecord new conda meta", |b| {
        b.iter(|| process_minimal_json_files_from_dir(&new_conda_meta));
    });
    serial_group.finish();

    // Parallel Processing Benchmarks (batch operations)
    let mut parallel_group = c.benchmark_group("Parallel Processing");
    parallel_group.bench_function("PrefixRecord", |b| {
        b.iter(|| load_as_prefix_record(&old_conda_meta_root));
    });
    parallel_group.bench_function("PackageRecord", |b| {
        b.iter(|| load_as_package_record(&old_conda_meta_root));
    });
    parallel_group.bench_function("MinimalPrefixRecord", |b| {
        b.iter(|| load_as_minimal_prefix_record(&old_conda_meta_root));
    });
    parallel_group.bench_function("MinimalPrefixRecord new conda meta", |b| {
        b.iter(|| load_as_minimal_prefix_record(&new_conda_meta_root));
    });
    parallel_group.finish();

    // Single File Benchmarks (normal size ~18MB file)
    if old_test_file.exists() {
        let mut single_file_group = c.benchmark_group("Single File");
        single_file_group.bench_function("PrefixRecord", |b| {
            b.iter(|| black_box(PrefixRecord::from_path(&old_test_file).unwrap()));
        });
        single_file_group.bench_function("PackageRecord", |b| {
            b.iter(|| black_box(PackageRecord::from_path(&old_test_file).unwrap()));
        });
        single_file_group.bench_function("MinimalPrefixRecord", |b| {
            b.iter(|| black_box(MinimalPrefixRecord::from_path(&old_test_file).unwrap()));
        });
        single_file_group.finish();
    }

    // Large File Benchmarks (synthetic ~20MB+ file with 20k entries)
    let mut large_file_group = c.benchmark_group("Large File");
    large_file_group.bench_function("PrefixRecord", |b| {
        b.iter(|| black_box(PrefixRecord::from_path(&new_long_file_path).unwrap()));
    });
    large_file_group.bench_function("PackageRecord", |b| {
        b.iter(|| black_box(PackageRecord::from_path(&new_long_file_path).unwrap()));
    });
    large_file_group.bench_function("MinimalPrefixRecord", |b| {
        b.iter(|| black_box(MinimalPrefixRecord::from_path(&new_long_file_path).unwrap()));
    });
    large_file_group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
