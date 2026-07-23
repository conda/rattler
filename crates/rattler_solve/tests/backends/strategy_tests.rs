//! Tests for solve strategy behavior (`LowestVersion`, `LowestVersionDirect`).

use super::helpers::{PackageBuilder, SolverCase};
use rattler_solve::{SolveStrategy, SolverImpl};

/// Test that `LowestVersion` strategy picks the lowest version but highest build number.
pub(super) fn solve_lowest_version_strategy<T: SolverImpl + Default>() {
    // Create multiple versions with different build numbers
    let pkg_v1_b1 = PackageBuilder::new("pkg")
        .version("1.0")
        .build_number(1)
        .build_string("build_1")
        .build();
    let pkg_v1_b2 = PackageBuilder::new("pkg")
        .version("1.0")
        .build_number(2)
        .build_string("build_2")
        .build();
    let pkg_v2 = PackageBuilder::new("pkg").version("2.0").build();

    SolverCase::new("lowest version strategy picks lowest version with highest build number")
        .repository([pkg_v1_b1, pkg_v1_b2, pkg_v2])
        .specs(["pkg"])
        .strategy(SolveStrategy::LowestVersion)
        .expect_present([("pkg", "1.0", "build_2")])
        .run::<T>();
}

/// Test that `LowestVersion` strategy applies to transitive dependencies too.
pub(super) fn solve_lowest_version_strategy_transitive<T: SolverImpl + Default>() {
    let dep_v1 = PackageBuilder::new("dep").version("1.0").build();
    let dep_v2 = PackageBuilder::new("dep").version("2.0").build();
    let main_pkg = PackageBuilder::new("main")
        .version("1.0")
        .depends(["dep"])
        .build();

    SolverCase::new("lowest version strategy applies to transitive deps")
        .repository([dep_v1, dep_v2, main_pkg])
        .specs(["main"])
        .strategy(SolveStrategy::LowestVersion)
        .expect_present([("main", "1.0"), ("dep", "1.0")])
        .run::<T>();
}

/// Test that `LowestVersionDirect` only applies lowest version to direct dependencies.
pub(super) fn solve_lowest_version_direct_strategy<T: SolverImpl + Default>() {
    let dep_v1 = PackageBuilder::new("dep").version("1.0").build();
    let dep_v2 = PackageBuilder::new("dep").version("2.0").build();
    let main_pkg = PackageBuilder::new("main")
        .version("1.0")
        .depends(["dep"])
        .build();

    SolverCase::new("lowest version direct only affects direct deps")
        .repository([dep_v1, dep_v2, main_pkg])
        .specs(["main"])
        .strategy(SolveStrategy::LowestVersionDirect)
        // main is lowest (direct), but dep should be highest (transitive)
        .expect_present([("main", "1.0"), ("dep", "2.0")])
        .run::<T>();
}
