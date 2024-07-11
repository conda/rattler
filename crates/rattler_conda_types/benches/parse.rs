use std::{fs, path::PathBuf};

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rattler_conda_types::{PrefixRecord, Version};

fn process_json_files_from_dir(dir: PathBuf) {
    let entries = fs::read_dir(dir).expect("Directory not found");

    for entry in entries {
        let entry = entry.expect("Unable to read entry");
        let path = entry.path();

        PrefixRecord::from_path(path).unwrap();
    }
}

fn criterion_benchmark(c: &mut Criterion) {
    // c.bench_function("parse simple version", |b| {
    //     b.iter(|| black_box("3.11.4").parse::<Version>());
    // });
    // c.bench_function("parse complex version", |b| {
    //     b.iter(|| black_box("1!1.0b2.post345.dev456+3.2.20.rc3").parse::<Version>());
    // });
    // c.bench_function("parse logical constraint", |b| {
    //     b.iter(|| black_box(">=3.1").parse::<Version>());
    // });
    // c.bench_function("parse wildcard constraint", |b| {
    //     b.iter(|| black_box("3.1.*").parse::<Version>());
    // });
    // c.bench_function("parse simple version spec", |b| {
    //     b.iter(|| black_box(">=3.1").parse::<Version>());
    // });
    // c.bench_function("parse complex version spec", |b| {
    //     b.iter(|| black_box("(>=2.1.0,<3.0)|(~=3.2.1,~3.2.2.1)|(==4.1)").parse::<Version>());
    // });
    c.bench_function("process_json_files", |b| {
        b.iter(|| {
            process_json_files_from_dir(black_box(black_box(
                "/Users/graf/projects/oss/rattler-1/test-data/conda-meta".into(),
            )))
        })
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
