//! Tests that the sorting of candidates remains the same.

use std::path::Path;

use futures::FutureExt;
use itertools::Itertools;
use rattler_conda_types::{
    Channel, MatchSpec, PackageName, ParseStrictness::Lenient, RepoDataRecord,
};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{resolvo::CondaDependencyProvider, ChannelPriority, SolveStrategy};
use resolvo::{Interner, SolverCache};
use rstest::*;

fn load_repodata(package_name: &PackageName) -> Vec<Vec<RepoDataRecord>> {
    let channel_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("test-data")
        .join("channels")
        .join("conda-forge");
    let repodata_json_path = channel_path.join("linux-64").join("repodata.json");
    let channel = Channel::from_directory(&channel_path);

    let sparse_repo_data = SparseRepoData::new(channel, "linux-64", repodata_json_path, None)
        .expect("failed to load sparse repodata");

    SparseRepoData::load_records_recursive(&[sparse_repo_data], [package_name.clone()], None)
        .expect("failed to load records")
}

fn create_sorting_snapshot(package_name: &str, strategy: SolveStrategy) -> String {
    let match_spec = MatchSpec::from_str(package_name, Lenient).unwrap();
    let package_name = match_spec.name.clone().unwrap();

    // Load repodata
    let repodata = load_repodata(&package_name);

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
        strategy,
    )
    .expect("failed to create dependency provider");

    let name = dependency_provider
        .pool
        .intern_package_name(package_name.as_normalized());
    let version_set = dependency_provider
        .pool
        .intern_version_set(name, match_spec.into_nameless().1.into());

    // Construct a cache
    let cache = SolverCache::new(dependency_provider);

    // Get the candidates for the package
    let sorted_candidates = cache
        .get_or_cache_sorted_candidates(version_set.into())
        .now_or_never()
        .expect("failed to get candidates")
        .expect("solver requested cancellation");

    sorted_candidates
        .iter()
        .map(|&candidate| cache.provider().display_solvable(candidate))
        .format("\n")
        .to_string()
}

#[rstest]
#[case::pytorch("pytorch >=1.12.0", SolveStrategy::Highest)]
#[case::pytorch("pytorch >=1.12.0", SolveStrategy::LowestVersion)]
#[case::pytorch("pytorch >=1.12.0", SolveStrategy::LowestVersionDirect)]
#[case::python("python ~=3.10.*", SolveStrategy::Highest)]
#[case::libuuid("libuuid", SolveStrategy::Highest)]
#[case::abess("abess", SolveStrategy::Highest)]
#[case::libgcc("libgcc-ng", SolveStrategy::Highest)]
#[case::certifi("certifi >=2016.9.26", SolveStrategy::Highest)]
fn test_ordering(#[case] spec: &str, #[case] solve_strategy: SolveStrategy) {
    insta::assert_snapshot!(
        format!(
            "test_ordering_{}_{}",
            spec.split_whitespace().next().unwrap_or(spec),
            match solve_strategy {
                SolveStrategy::Highest => "highest",
                SolveStrategy::LowestVersion => "lowest",
                SolveStrategy::LowestVersionDirect => "lowest_direct",
            }
        ),
        create_sorting_snapshot(spec, solve_strategy)
    );
}
