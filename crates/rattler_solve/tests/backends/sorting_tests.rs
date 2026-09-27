//! Tests for how candidates of the same version and build number are ordered.

use super::helpers::{PackageBuilder, SolverCase};
use rattler_conda_types::RepoDataRecord;
use rattler_solve::SolverImpl;

/// Two builds of `pkg 1.0` with the same build number that pin different majors
/// of `dep`. Mirrors conda-forge, where `pnpm 11.19.0` exists both pinned to
/// `nodejs 24.*` and pinned to `nodejs 26.*`.
///
/// With `bare_requirement` both builds also carry an unpinned `dep` next to the
/// pin, which is what a recipe listing `dep` in its run requirements produces.
fn duplicate_dependency_repository(bare_requirement: bool) -> Vec<RepoDataRecord> {
    let dep_24 = PackageBuilder::new("dep").version("24.19.0").build();
    let dep_26 = PackageBuilder::new("dep").version("26.6.0").build();

    let requirements = |pin: &str| {
        if bare_requirement {
            vec!["dep".to_string(), pin.to_string()]
        } else {
            vec![pin.to_string()]
        }
    };

    // The build pinned to the older `dep` is built last, so it wins any tie that
    // falls through to the timestamp.
    let pkg_dep24 = PackageBuilder::new("pkg")
        .version("1.0")
        .build_number(0)
        .build_string("h_dep24_0")
        .depends(requirements("dep >=24.18.0,<25.0a0"))
        .timestamp("2026-07-31T18:34:43Z")
        .build();
    let pkg_dep26 = PackageBuilder::new("pkg")
        .version("1.0")
        .build_number(0)
        .build_string("h_dep26_0")
        .depends(requirements("dep >=26.5.1,<27.0a0"))
        .timestamp("2026-07-31T18:34:41Z")
        .build();

    vec![dep_24, dep_26, pkg_dep24, pkg_dep26]
}

/// One pin per build: the build allowing the newest `dep` wins, even though the
/// other one has a newer timestamp.
pub(super) fn solve_prefers_build_with_highest_dependency<T: SolverImpl + Default>() {
    SolverCase::new("build pinning the newest dependency wins over a newer build timestamp")
        .repository(duplicate_dependency_repository(false))
        .specs(["pkg"])
        .expect_present([("pkg", "1.0", "h_dep26_0")])
        .expect_present([("dep", "26.6.0")])
        .run::<T>();
}

/// Same, but with a bare `dep` next to each pin. The bare requirement matches
/// every `dep`, so scoring a build by its least restrictive requirement leaves
/// the timestamp to decide, and that only says which variant built last.
pub(super) fn solve_prefers_build_with_highest_dependency_with_bare_requirement<
    T: SolverImpl + Default,
>() {
    SolverCase::new("a bare requirement next to a pin does not mask the pin")
        .repository(duplicate_dependency_repository(true))
        .specs(["pkg"])
        .expect_present([("pkg", "1.0", "h_dep26_0")])
        .expect_present([("dep", "26.6.0")])
        .run::<T>();
}
