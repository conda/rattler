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

fn criterion_benchmark(c: &mut Criterion) {
    let test_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/conda-meta");
    let test_file = test_dir.join("rust-1.88.0-h1a8d7c4_0.json");

    let mut super_long_file: PrefixRecord = PrefixRecord::from_path(&test_file).unwrap();
    // duplicate data until we have 20k paths
    let files = super_long_file.files.clone();
    while super_long_file.files.len() < 20_000 {
        super_long_file.files.extend(files.clone());
    }

    // Create a properly named temp file that all long file benchmarks can use
    let temp_dir = tempfile::tempdir().unwrap();
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
    let long_file_path = temp_dir.path().join(filename);
    serde_json::to_writer(File::create(&long_file_path).unwrap(), &super_long_file).unwrap();

    // Serial Processing Benchmarks (one file at a time)
    let mut serial_group = c.benchmark_group("Serial Processing");
    serial_group.bench_function("PrefixRecord", |b| {
        b.iter(|| process_json_files_from_dir(&test_dir));
    });
    serial_group.bench_function("MinimalPrefixRecord", |b| {
        b.iter(|| process_minimal_json_files_from_dir(&test_dir));
    });
    serial_group.bench_function("PackageRecord", |b| {
        b.iter(|| process_package_record_files_from_dir(&test_dir));
    });
    serial_group.finish();

    // Parallel Processing Benchmarks (batch operations)
    let mut parallel_group = c.benchmark_group("Parallel Processing");
    parallel_group.bench_function("PrefixRecord", |b| {
        b.iter(|| load_as_prefix_record(&test_dir));
    });
    parallel_group.bench_function("MinimalPrefixRecord", |b| {
        b.iter(|| load_as_minimal_prefix_record(&test_dir));
    });
    parallel_group.bench_function("PackageRecord", |b| {
        b.iter(|| load_as_package_record(&test_dir));
    });
    parallel_group.finish();

    // Single File Benchmarks (normal size ~18MB file)
    if test_file.exists() {
        let mut single_file_group = c.benchmark_group("Single File");
        single_file_group.bench_function("PrefixRecord", |b| {
            b.iter(|| black_box(PrefixRecord::from_path(&test_file).unwrap()));
        });
        single_file_group.bench_function("MinimalPrefixRecord", |b| {
            b.iter(|| black_box(MinimalPrefixRecord::from_path(&test_file).unwrap()));
        });
        single_file_group.bench_function("PackageRecord", |b| {
            b.iter(|| black_box(PackageRecord::from_path(&test_file).unwrap()));
        });
        single_file_group.finish();
    }

    // Large File Benchmarks (synthetic ~20MB+ file with 20k entries)
    let mut large_file_group = c.benchmark_group("Large File");
    large_file_group.bench_function("PrefixRecord", |b| {
        b.iter(|| black_box(PrefixRecord::from_path(&long_file_path).unwrap()));
    });
    large_file_group.bench_function("MinimalPrefixRecord", |b| {
        b.iter(|| black_box(MinimalPrefixRecord::from_path(&long_file_path).unwrap()));
    });
    large_file_group.bench_function("PackageRecord", |b| {
        b.iter(|| black_box(PackageRecord::from_path(&long_file_path).unwrap()));
    });
    large_file_group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
