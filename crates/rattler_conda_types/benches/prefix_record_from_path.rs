use rattler_conda_types::RecordFromPath;
use std::{
    fs::{self},
    path::{Path, PathBuf},
};

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rattler_conda_types::{PackageRecord, PrefixRecord};

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

fn criterion_benchmark(c: &mut Criterion) {
    let test_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/conda-meta");

    let mut super_long_file: PrefixRecord =
        PrefixRecord::from_path(test_dir.join("tk-8.6.13-h5083fa2_1.json")).unwrap();
    // duplicate data until we have 20k paths
    let files = super_long_file.files.clone();
    while super_long_file.files.len() < 20_000 {
        super_long_file.files.extend(files.clone());
    }

    let tempfile = tempfile::NamedTempFile::new().unwrap();
    serde_json::to_writer(&tempfile, &super_long_file).unwrap();

    c.bench_function("load_prefix_record_serially", |b| {
        b.iter(|| {
            process_json_files_from_dir(&test_dir);
        });
    });
    c.bench_function("load_as_prefix_record", |b| {
        b.iter(|| load_as_prefix_record(&test_dir));
    });
    c.bench_function("load_as_package_record", |b| {
        b.iter(|| load_as_package_record(&test_dir));
    });

    let path = tempfile.path();
    c.bench_function("load_long_prefix_record", |b| {
        b.iter(|| black_box(PrefixRecord::from_path(path).unwrap()));
    });

    c.bench_function("load_long_package_record", |b| {
        b.iter(|| {
            black_box(PackageRecord::from_path(path).unwrap());
        });
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
