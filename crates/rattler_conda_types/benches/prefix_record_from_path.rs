use std::{
    fs,
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
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
