use std::path::Path;

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use futures::FutureExt;
use rattler_conda_types::{Channel, MatchSpec};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::resolvo::CondaDependencyProvider;
use rattler_solve::ChannelPriority;
use resolvo::SolverCache;

fn bench_sort(c: &mut Criterion, sparse_repo_data: &SparseRepoData, spec: &str) {
    let match_spec =
        MatchSpec::from_str(spec, rattler_conda_types::ParseStrictness::Lenient).unwrap();
    let package_name = match_spec.name.clone().unwrap();

    let repodata =
        SparseRepoData::load_records_recursive([sparse_repo_data], [package_name.clone()], None)
            .expect("failed to load records");

    // Construct a cache
    c.bench_function(&format!("sort {spec}"), |b| {
        // Get the candidates for the package
        b.iter_batched(
            || (package_name.clone(), match_spec.clone()),
            |(package_name, match_spec)| {
                // Construct dependency provider
                let dependency_provider = CondaDependencyProvider::new(
                    repodata.iter().map(|r| r.iter().collect()),
                    &[],
                    &[],
                    &[],
                    &[match_spec.clone()],
                    None,
                    ChannelPriority::default(),
                    None,
                    rattler_solve::SolveStrategy::Highest,
                )
                .expect("failed to create dependency provider");

                let name = dependency_provider
                    .pool
                    .intern_package_name(package_name.as_normalized());
                let version_set = dependency_provider
                    .pool
                    .intern_version_set(name, match_spec.into_nameless().1.into());

                let cache = SolverCache::new(dependency_provider);

                let deps = cache
                    .get_or_cache_sorted_candidates(version_set.into())
                    .now_or_never()
                    .expect("failed to get candidates")
                    .expect("solver requested cancellation");
                black_box(deps);
            },
            BatchSize::SmallInput,
        );
    });
}

fn criterion_benchmark(c: &mut Criterion) {
    let channel_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("test-data")
        .join("channels")
        .join("conda-forge");
    let repodata_json_path = channel_path.join("linux-64").join("repodata.json");
    let channel = Channel::from_directory(&channel_path);

    let sparse_repo_data = SparseRepoData::new(channel, "linux-64", repodata_json_path, None)
        .expect("failed to load sparse repodata");

    bench_sort(c, &sparse_repo_data, "pytorch");
    bench_sort(c, &sparse_repo_data, "python");
    bench_sort(c, &sparse_repo_data, "tensorflow");
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
