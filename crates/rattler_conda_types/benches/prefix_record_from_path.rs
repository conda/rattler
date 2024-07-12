use std::{fs, path::PathBuf};

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rattler_conda_types::PrefixRecord;

fn process_json_files_from_dir(dir: PathBuf) {
    let entries = fs::read_dir(dir).expect("Directory not found");

    for entry in entries {
        let entry = entry.expect("Unable to read entry");
        let path = entry.path();

        PrefixRecord::from_path(path).unwrap();
    }
}

fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("process_json_files", |b| {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        b.iter(|| {
            process_json_files_from_dir(black_box(manifest_dir.join("../../test-data/conda-meta")));
        });
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
