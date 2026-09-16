//! Measures what `SolverTask::excluded_candidates` costs a solve.
//!
//! The offline case is the extreme one: nearly every candidate is excluded,
//! so the map holds one owned URL and the solver performs one lookup per
//! interned record. To keep the runs comparable, the exclusions never remove
//! anything the solution needs: the solution records stay allowed, and the
//! given share of the remaining candidates is ruled out. Every run therefore
//! solves to the same result and differs only in the bookkeeping.

use std::{collections::HashMap, hint::black_box, sync::Arc};

use criterion::{Criterion, SamplingMode, criterion_group, criterion_main};
use rattler_conda_types::ParseStrictness::Strict;
use rattler_conda_types::{Channel, ChannelConfig, MatchSpec, RepoDataRecord};
use rattler_repodata_gateway::sparse::{PackageFormatSelection, SparseRepoData};
use rattler_solve::{SolverImpl, SolverTask};
use url::Url;

fn conda_json_path() -> String {
    format!(
        "{}/{}",
        env!("CARGO_MANIFEST_DIR"),
        "../../test-data/channels/conda-forge/linux-64/repodata.json"
    )
}

fn conda_json_path_noarch() -> String {
    format!(
        "{}/{}",
        env!("CARGO_MANIFEST_DIR"),
        "../../test-data/channels/conda-forge/noarch/repodata.json"
    )
}

fn read_sparse_repodata(path: &str) -> SparseRepoData {
    SparseRepoData::from_file(
        Channel::from_str(
            "dummy",
            &ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap()),
        )
        .unwrap(),
        "dummy".to_string(),
        path,
        None,
    )
    .unwrap()
}

/// Excludes the given share of candidates while keeping every record of the
/// unrestricted solution allowed, so the solve still succeeds identically.
fn exclusions(
    available: &[Vec<RepoDataRecord>],
    solution_urls: &std::collections::HashSet<Url>,
    share: f64,
) -> HashMap<Url, Arc<str>> {
    let reason: Arc<str> = Arc::from("not available locally");
    let excludable: Vec<&RepoDataRecord> = available
        .iter()
        .flatten()
        .filter(|record| !solution_urls.contains(&record.url))
        .collect();
    let count = (excludable.len() as f64 * share) as usize;
    excludable
        .into_iter()
        .take(count)
        .map(|record| (record.url.clone(), Arc::clone(&reason)))
        .collect()
}

fn bench_solve_with_exclusions(c: &mut Criterion, spec: &str) {
    let mut group = c.benchmark_group(format!("solve {spec} with exclusions"));
    group.sampling_mode(SamplingMode::Flat);
    group.sample_size(20);

    let specs = vec![MatchSpec::from_str(spec, Strict).unwrap()];

    let json_file = conda_json_path();
    let json_file_noarch = conda_json_path_noarch();
    let sparse_repo_data = vec![
        read_sparse_repodata(&json_file),
        read_sparse_repodata(&json_file_noarch),
    ];

    let names = specs.iter().map(|s| s.name.as_exact().unwrap().clone());
    let available_packages = SparseRepoData::load_records_recursive(
        &sparse_repo_data,
        names,
        None,
        PackageFormatSelection::default(),
    )
    .unwrap();

    let solution_urls: std::collections::HashSet<Url> = rattler_solve::resolvo::Solver
        .solve(SolverTask {
            specs: specs.clone(),
            ..SolverTask::from_iter(&available_packages)
        })
        .unwrap()
        .records
        .into_iter()
        .map(|record| record.url)
        .collect();

    for (label, share) in [
        ("empty", 0.0),
        ("90% excluded", 0.9),
        ("99% excluded", 0.99),
    ] {
        // Building the map is part of what a caller pays per solve, so it is
        // measured on its own; the solve is then timed with the map handed
        // over outside the measurement.
        group.bench_function(format!("build map, {label}"), |b| {
            b.iter(|| black_box(exclusions(&available_packages, &solution_urls, share)));
        });

        let excluded_candidates = exclusions(&available_packages, &solution_urls, share);
        group.bench_function(format!("solve, {label}"), |b| {
            b.iter_batched(
                || excluded_candidates.clone(),
                |excluded_candidates| {
                    rattler_solve::resolvo::Solver
                        .solve(black_box(SolverTask {
                            specs: specs.clone(),
                            excluded_candidates,
                            ..SolverTask::from_iter(&available_packages)
                        }))
                        .unwrap()
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

fn criterion_benchmark(c: &mut Criterion) {
    bench_solve_with_exclusions(c, "python=3.9");
    bench_solve_with_exclusions(c, "quetz");
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
