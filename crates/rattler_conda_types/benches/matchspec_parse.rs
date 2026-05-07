use std::collections::HashSet;
use std::hint::black_box;
use std::path::Path;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use rattler_conda_types::{MatchSpec, NamelessMatchSpec, ParseStrictness, RepoData};

/// Load repodata and collect all unique dependency/constraint strings.
fn load_unique_specs() -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data/channels/conda-forge/linux-64/repodata.json");
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read repodata at {}: {e}", path.display()));
    let repodata: RepoData =
        serde_json::from_str(&contents).unwrap_or_else(|e| panic!("failed to parse repodata: {e}"));

    let mut unique = HashSet::new();
    for record in repodata
        .packages
        .values()
        .chain(repodata.conda_packages.values())
    {
        for dep in &record.depends {
            unique.insert(dep.clone());
        }
        for con in &record.constrains {
            unique.insert(con.clone());
        }
    }

    let mut specs: Vec<String> = unique.into_iter().collect();
    specs.sort();
    specs
}

/// Extract the version/build portion of a spec string (everything after the
/// first space), suitable for `NamelessMatchSpec` parsing.
fn version_build_portions(specs: &[String]) -> Vec<String> {
    specs
        .iter()
        .filter_map(|s| {
            let trimmed = s.trim();
            trimmed
                .find(char::is_whitespace)
                .map(|pos| trimmed[pos..].trim().to_string())
        })
        .collect()
}

fn matchspec_benchmarks(c: &mut Criterion) {
    let specs = load_unique_specs();
    let nameless_inputs = version_build_portions(&specs);

    let spec_count = specs.len() as u64;
    let nameless_count = nameless_inputs.len() as u64;

    eprintln!("Loaded {spec_count} unique MatchSpec strings, {nameless_count} nameless portions");

    // --- MatchSpec benchmarks ---
    let mut group = c.benchmark_group("MatchSpec");
    group.throughput(Throughput::Elements(spec_count));
    group.sample_size(20);

    group.bench_function("lenient", |b| {
        b.iter(|| {
            for spec in &specs {
                let _ = black_box(MatchSpec::from_str(
                    black_box(spec),
                    ParseStrictness::Lenient,
                ));
            }
        });
    });

    group.bench_function("strict", |b| {
        b.iter(|| {
            for spec in &specs {
                let _ = black_box(MatchSpec::from_str(
                    black_box(spec),
                    ParseStrictness::Strict,
                ));
            }
        });
    });

    // Specs that fail strict, parsed as lenient (isolates fallback cost)
    let strict_failures: Vec<&String> = specs
        .iter()
        .filter(|s| MatchSpec::from_str(s, ParseStrictness::Strict).is_err())
        .collect();
    let failure_count = strict_failures.len() as u64;

    if failure_count > 0 {
        eprintln!("{failure_count} specs fail strict parsing");
        group.throughput(Throughput::Elements(failure_count));
        group.bench_function("strict-failures-lenient", |b| {
            b.iter(|| {
                for spec in &strict_failures {
                    let _ = black_box(MatchSpec::from_str(
                        black_box(spec.as_str()),
                        ParseStrictness::Lenient,
                    ));
                }
            });
        });
    }

    group.finish();

    // --- NamelessMatchSpec benchmarks ---
    let mut group = c.benchmark_group("NamelessMatchSpec");
    group.throughput(Throughput::Elements(nameless_count));
    group.sample_size(20);

    group.bench_function("lenient", |b| {
        b.iter(|| {
            for input in &nameless_inputs {
                let _ = black_box(NamelessMatchSpec::from_str(
                    black_box(input),
                    ParseStrictness::Lenient,
                ));
            }
        });
    });

    group.bench_function("strict", |b| {
        b.iter(|| {
            for input in &nameless_inputs {
                let _ = black_box(NamelessMatchSpec::from_str(
                    black_box(input),
                    ParseStrictness::Strict,
                ));
            }
        });
    });

    group.finish();
}

criterion_group!(benches, matchspec_benchmarks);
criterion_main!(benches);
