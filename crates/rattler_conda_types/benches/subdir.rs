use std::collections::HashSet;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use rattler_conda_types::Subdir;

/// A mix of subdir-shaped strings as they show up in URLs, match specs and
/// lock files: known subdirs, unknown CEP 26 subdirs and channel names.
const INPUTS: &[&str] = &[
    "linux-64",
    "noarch",
    "osx-arm64",
    "win-64",
    "linux-aarch64",
    "emscripten-wasm32",
    "zos-z",
    "linux-esp32s3",
    "conda-forge",
    "bioconda",
    "pkgs",
    "main",
];

fn bench_subdir(c: &mut Criterion) {
    let known: Vec<Subdir> = Subdir::known().collect();

    c.bench_function("subdir/from_known_str", |b| {
        b.iter(|| {
            for input in INPUTS {
                black_box(Subdir::from_known_str(black_box(input)));
            }
        });
    });

    c.bench_function("subdir/from_str", |b| {
        b.iter(|| {
            for input in INPUTS {
                let _ = black_box(black_box(*input).parse::<Subdir>());
            }
        });
    });

    c.bench_function("subdir/is_known", |b| {
        let subdirs: Vec<Subdir> = INPUTS.iter().filter_map(|s| s.parse().ok()).collect();
        b.iter(|| {
            for subdir in &subdirs {
                black_box(black_box(subdir).is_known());
            }
        });
    });

    c.bench_function("subdir/as_str", |b| {
        b.iter(|| {
            for subdir in &known {
                black_box(black_box(subdir).as_str());
            }
        });
    });

    c.bench_function("subdir/is_unix", |b| {
        b.iter(|| {
            for subdir in &known {
                black_box(black_box(subdir).is_unix());
            }
        });
    });

    c.bench_function("subdir/eq", |b| {
        b.iter(|| {
            for subdir in &known {
                black_box(*black_box(subdir) == Subdir::Linux64);
            }
        });
    });

    c.bench_function("subdir/sort", |b| {
        b.iter(|| {
            let mut subdirs = known.clone();
            subdirs.reverse();
            subdirs.sort();
            black_box(subdirs);
        });
    });

    c.bench_function("subdir/hashset_lookup", |b| {
        let set: HashSet<Subdir> = known.iter().copied().collect();
        b.iter(|| {
            for subdir in &known {
                black_box(set.contains(black_box(subdir)));
            }
        });
    });

    c.bench_function("subdir/deserialize_json", |b| {
        let json = serde_json::to_string(&known).unwrap();
        b.iter(|| {
            let subdirs: Vec<Subdir> = serde_json::from_str(black_box(&json)).unwrap();
            black_box(subdirs);
        });
    });
}

criterion_group!(benches, bench_subdir);
criterion_main!(benches);
