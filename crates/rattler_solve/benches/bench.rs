use criterion::{black_box, criterion_group, criterion_main, Criterion, SamplingMode};
use rattler_conda_types::ParseStrictness::Strict;
use rattler_conda_types::{Channel, ChannelConfig, MatchSpec};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{SolverImpl, SolverTask};

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
    SparseRepoData::new(
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

fn bench_solve_environment(c: &mut Criterion, specs: Vec<&str>) {
    let name = specs.join(", ");
    let mut group = c.benchmark_group(format!("solve {name}"));

    group.sampling_mode(SamplingMode::Flat);
    group.sample_size(20);

    let specs = specs
        .iter()
        .map(|s| MatchSpec::from_str(s, Strict).unwrap())
        .collect::<Vec<MatchSpec>>();

    let json_file = conda_json_path();
    let json_file_noarch = conda_json_path_noarch();

    let sparse_repo_data = vec![
        read_sparse_repodata(&json_file),
        read_sparse_repodata(&json_file_noarch),
    ];

    let names = specs.iter().map(|s| s.name.clone().unwrap());
    let available_packages =
        SparseRepoData::load_records_recursive(&sparse_repo_data, names, None).unwrap();

    #[cfg(feature = "libsolv_c")]
    group.bench_function("libsolv_c", |b| {
        b.iter(|| {
            rattler_solve::libsolv_c::Solver
                .solve(black_box(SolverTask {
                    specs: specs.clone(),
                    ..SolverTask::from_iter(&available_packages)
                }))
                .unwrap()
        });
    });

    #[cfg(feature = "resolvo")]
    group.bench_function("resolvo", |b| {
        b.iter(|| {
            rattler_solve::resolvo::Solver
                .solve(black_box(SolverTask {
                    specs: specs.clone(),
                    ..SolverTask::from_iter(&available_packages)
                }))
                .unwrap()
        });
    });

    group.finish();
}

fn criterion_benchmark(c: &mut Criterion) {
    bench_solve_environment(c, vec!["python=3.9"]);
    bench_solve_environment(c, vec!["xtensor", "xsimd"]);
    bench_solve_environment(c, vec!["tensorflow"]);
    bench_solve_environment(c, vec!["quetz"]);
    bench_solve_environment(c, vec!["tensorboard=2.1.1", "grpc-cpp=1.39.1"]);
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
