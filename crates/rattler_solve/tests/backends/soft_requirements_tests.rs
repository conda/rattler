//! Tests for `soft_requirements` functionality.

use super::helpers::{PackageBuilder, SolverCase};
use rattler_solve::SolverImpl;

/// Test that soft requirements are installed when they can be satisfied.
pub(super) fn solve_soft_requirements_basic<T: SolverImpl + Default>() {
    let foo_1 = PackageBuilder::new("foo").version("1.0").build();
    let bar_1 = PackageBuilder::new("bar").version("1.0").build();

    SolverCase::new("soft requirements are installed when possible")
        .repository([foo_1, bar_1])
        .specs(["foo"])
        .soft_requirements(["bar"])
        .expect_present([("foo", "1.0"), ("bar", "1.0")])
        .run::<T>();
}

/// Test that unsatisfiable soft requirements don't cause solve failure.
pub(super) fn solve_soft_requirements_unsatisfiable<T: SolverImpl + Default>() {
    let foo_1 = PackageBuilder::new("foo").version("1.0").build();

    SolverCase::new("unsatisfiable soft requirements don't cause failure")
        .repository([foo_1])
        .specs(["foo"])
        .soft_requirements(["nonexistent"])
        .expect_present([("foo", "1.0")])
        .run::<T>();
}

/// Test that soft requirements respect hard constraints from specs.
pub(super) fn solve_soft_requirements_conflict<T: SolverImpl + Default>() {
    let foo_1 = PackageBuilder::new("foo")
        .version("1.0")
        .depends(["bar<2"])
        .build();
    let bar_1 = PackageBuilder::new("bar").version("1.0").build();
    let bar_2 = PackageBuilder::new("bar").version("2.0").build();

    // Soft requirement for bar>=2 can't be satisfied due to foo's constraint
    SolverCase::new("soft requirements respect hard constraints")
        .repository([foo_1, bar_1, bar_2])
        .specs(["foo"])
        .soft_requirements(["bar>=2"])
        .expect_present([("foo", "1.0"), ("bar", "1.0")])
        .run::<T>();
}

/// Test that soft requirements with version constraints work.
pub(super) fn solve_soft_requirements_versioned<T: SolverImpl + Default>() {
    let foo_1 = PackageBuilder::new("foo").version("1.0").build();
    let bar_1 = PackageBuilder::new("bar").version("1.0").build();
    let bar_2 = PackageBuilder::new("bar").version("2.0").build();

    SolverCase::new("soft requirements with version constraints")
        .repository([foo_1, bar_1, bar_2])
        .specs(["foo"])
        .soft_requirements(["bar>=2"])
        .expect_present([("foo", "1.0"), ("bar", "2.0")])
        .run::<T>();
}
