//! Tests migrated to use the `SolverCase` helper for improved readability and consistency.

use super::helpers::{PackageBuilder, SolverCase};
use rattler_solve::SolverImpl;

/// Test that already-installed packages are favored when solving.
pub(super) fn solve_favored<T: SolverImpl + Default>() {
    let bors_1 = PackageBuilder::new("bors").version("1.0").build();
    let bors_2 = PackageBuilder::new("bors").version("2.0").build();

    SolverCase::new("bors is favored when already installed")
        .repository([bors_1.clone(), bors_2])
        .specs(["bors"])
        .locked_packages([bors_1.clone()])
        .expect_present([("bors", "1.0")])
        .run::<T>();
}

/// Test that constraints limit which package versions can be selected.
pub(super) fn solve_constraints<T: SolverImpl + Default>() {
    let bors_1 = PackageBuilder::new("bors").version("1.0").build();
    let bors_2 = PackageBuilder::new("bors").version("2.0").build();
    let foobar = PackageBuilder::new("foobar")
        .version("1.0")
        .depends(["bors"])
        .build();

    SolverCase::new("constraints limit package selection")
        .repository([bors_1, bors_2, foobar])
        .specs(["foobar"])
        .constraints(["bors <=1"])
        .expect_present([("bors", "1.0"), ("foobar", "1.0")])
        .run::<T>();
}

/// Test that `exclude_newer` filters out packages newer than the given timestamp.
pub(super) fn solve_exclude_newer<T: SolverImpl + Default>() {
    let foo_old = PackageBuilder::new("foo")
        .version("1.0")
        .timestamp("2021-06-01T00:00:00Z")
        .build();
    let foo_new = PackageBuilder::new("foo")
        .version("2.0")
        .timestamp("2022-06-01T00:00:00Z")
        .build();

    SolverCase::new("exclude_newer filters out packages newer than timestamp")
        .repository([foo_old, foo_new])
        .specs(["foo"])
        .exclude_newer("2022-01-01T00:00:00Z")
        .expect_present([("foo", "1.0")])
        .run::<T>();
}

/// Test upgrading a package to a newer version.
pub(super) fn solve_upgrade<T: SolverImpl + Default>() {
    let foo_1 = PackageBuilder::new("foo").version("1.0").build();
    let foo_2 = PackageBuilder::new("foo").version("2.0").build();

    SolverCase::new("upgrade foo from 1.0 to 2.0")
        .repository([foo_1.clone(), foo_2])
        .specs(["foo>=2"])
        .locked_packages([foo_1])
        .expect_present([("foo", "2.0")])
        .run::<T>();
}

/// Test downgrading a package to an older version.
pub(super) fn solve_downgrade<T: SolverImpl + Default>() {
    let foo_1 = PackageBuilder::new("foo").version("1.0").build();
    let foo_2 = PackageBuilder::new("foo").version("2.0").build();

    SolverCase::new("downgrade foo from 2.0 to 1.0")
        .repository([foo_1, foo_2.clone()])
        .specs(["foo<2"])
        .locked_packages([foo_2])
        .expect_present([("foo", "1.0")])
        .run::<T>();
}

/// Test that installing a new package works correctly.
pub(super) fn solve_install_new<T: SolverImpl + Default>() {
    let foo_1 = PackageBuilder::new("foo").version("1.0").build();
    let foo_2 = PackageBuilder::new("foo").version("2.0").build();

    SolverCase::new("install picks highest matching version")
        .repository([foo_1, foo_2])
        .specs(["foo<2"])
        .expect_present([("foo", "1.0")])
        .run::<T>();
}

/// Test that removing a package results in empty solution.
#[allow(clippy::disallowed_names)]
pub(super) fn solve_remove<T: SolverImpl + Default>() {
    let foo = PackageBuilder::new("foo").version("1.0").build();

    SolverCase::new("removing a package results in empty solution")
        .repository([foo.clone()])
        .locked_packages([foo])
        .expect_absent(["foo"])
        .run::<T>();
}

/// Test that already-installed packages are kept when still satisfying specs.
pub(super) fn solve_noop<T: SolverImpl + Default>() {
    let foo_1 = PackageBuilder::new("foo").version("1.0").build();
    let foo_2 = PackageBuilder::new("foo").version("2.0").build();

    SolverCase::new("keep installed package when it satisfies spec")
        .repository([foo_1.clone(), foo_2])
        .specs(["foo<2"])
        .locked_packages([foo_1])
        .expect_present([("foo", "1.0")])
        .run::<T>();
}
