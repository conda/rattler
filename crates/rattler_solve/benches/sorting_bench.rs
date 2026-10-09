use std::{collections::HashMap, hint::black_box, path::Path};

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use futures::FutureExt;
use rattler_conda_types::{Channel, MatchSpec};
use rattler_repodata_gateway::sparse::{PackageFormatSelection, SparseRepoData};
use rattler_solve::{
    ChannelPriority,
    resolvo::{CondaDependencyProvider, NameType},
};
use resolvo::{DependencyProvider, SolverCache};

fn bench_filter(c: &mut Criterion, sparse_repo_data: &SparseRepoData, spec: &str) {
    let spec = MatchSpec::from_str(spec, rattler_conda_types::ParseStrictness::Lenient).unwrap();
    let name = spec.name.as_exact().unwrap().clone();
    let repodata = SparseRepoData::load_records_recursive(
        [sparse_repo_data],
        [name.clone()],
        None,
        PackageFormatSelection::default(),
    )
    .unwrap();
    let provider = CondaDependencyProvider::new(
        repodata.iter().map(|r| r.iter().collect()),
        &[],
        &[],
        &[],
        std::slice::from_ref(&spec),
        None,
        None,
        ChannelPriority::default(),
        None,
        rattler_solve::SolveStrategy::Highest,
        Vec::new(),
        &HashMap::default(),
    )
    .unwrap();
    let name_id = provider.pool.intern_package_name(NameType::from(&name));
    let version_set = provider
        .pool
        .intern_version_set(name_id, spec.clone().into_nameless().1.into());
    let candidates = provider
        .get_candidates(name_id)
        .now_or_never()
        .unwrap()
        .unwrap()
        .candidates;
    eprintln!("filter {spec}: {} candidates", candidates.len());
    for inverse in [false, true] {
        c.bench_function(&format!("filter {spec} inverse={inverse}"), |b| {
            b.iter(|| {
                black_box(
                    provider
                        .filter_candidates(black_box(&candidates), version_set, inverse)
                        .now_or_never()
                        .unwrap(),
                )
            });
        });
    }
}

fn bench_sort(c: &mut Criterion, sparse_repo_data: &SparseRepoData, spec: &str) {
    let match_spec =
        MatchSpec::from_str(spec, rattler_conda_types::ParseStrictness::Lenient).unwrap();
    let package_name = match_spec.name.as_exact().unwrap().clone();

    let repodata = SparseRepoData::load_records_recursive(
        [sparse_repo_data],
        [package_name.clone()],
        None,
        PackageFormatSelection::default(),
    )
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
                    std::slice::from_ref(&match_spec),
                    None,
                    None,
                    ChannelPriority::default(),
                    None,
                    rattler_solve::SolveStrategy::Highest,
                    Vec::new(),
                    &HashMap::default(),
                )
                .expect("failed to create dependency provider");

                let name = dependency_provider
                    .pool
                    .intern_package_name(NameType::from(&package_name));
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
    let channel = Channel::try_from_directory(&channel_path).unwrap();

    let sparse_repo_data = SparseRepoData::from_file(channel, "linux-64", repodata_json_path, None)
        .expect("failed to load sparse repodata");

    bench_sort(c, &sparse_repo_data, "pytorch");
    bench_sort(c, &sparse_repo_data, "python");
    bench_sort(c, &sparse_repo_data, "tensorflow");
    for spec in [
        "python >=3.9,<3.10",
        "numpy >=1.20,<2",
        "numpy >=1.20,<2 py39*",
        "libblas * *openblas",
        "python",
    ] {
        bench_filter(c, &sparse_repo_data, spec);
    }
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
