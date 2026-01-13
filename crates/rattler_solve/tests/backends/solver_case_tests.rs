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
    use super::helpers::run_solver_cases;

    let bors_1 = PackageBuilder::new("bors").version("1.0").build();
    let bors_2 = PackageBuilder::new("bors").version("2.0").build();
    let foobar = PackageBuilder::new("foobar")
        .version("1.0")
        .depends(["bors"])
        .build();

    run_solver_cases::<T>(&[
        SolverCase::new("constraints limit package selection")
            .repository([bors_1.clone(), bors_2.clone(), foobar.clone()])
            .specs(["foobar"])
            .constraints(["bors <=1"])
            .expect_present([("bors", "1.0"), ("foobar", "1.0")]),
        // Test that constraints on non-existing packages don't cause errors
        SolverCase::new("constraints on non-existing packages are allowed")
            .repository([bors_1, bors_2, foobar])
            .specs(["foobar"])
            .constraints(["bors <=1", "nonexisting"])
            .expect_present([("bors", "1.0"), ("foobar", "1.0")]),
    ]);
}

/// Test that `exclude_newer` filters out packages newer than the given timestamp.
pub(super) fn solve_exclude_newer<T: SolverImpl + Default>() {
    use super::helpers::run_solver_cases;
    use rattler_conda_types::package::ArchiveType;

    // Basic version filtering
    let foo_old = PackageBuilder::new("foo")
        .version("1.0")
        .timestamp("2021-06-01T00:00:00Z")
        .build();
    let foo_new = PackageBuilder::new("foo")
        .version("2.0")
        .timestamp("2022-06-01T00:00:00Z")
        .build();

    // Archive type selection based on timestamp:
    // When both .conda and .tar.bz2 exist for same version but .conda is newer,
    // exclude_newer should cause .tar.bz2 to be selected
    let foo_tarbz2 = PackageBuilder::new("foo")
        .version("3.0")
        .build_string("build_1")
        .archive_type(ArchiveType::TarBz2)
        .timestamp("2021-06-01T00:00:00Z")
        .build();
    let foo_conda = PackageBuilder::new("foo")
        .version("3.0")
        .build_string("build_1")
        .archive_type(ArchiveType::Conda)
        .timestamp("2022-06-01T00:00:00Z")
        .build();
    let foo_newer_version = PackageBuilder::new("foo")
        .version("4.0")
        .timestamp("2022-06-01T00:00:00Z")
        .build();

    run_solver_cases::<T>(&[
        SolverCase::new("exclude_newer filters out packages newer than timestamp")
            .repository([foo_old, foo_new])
            .specs(["foo"])
            .exclude_newer("2022-01-01T00:00:00Z")
            .expect_present([("foo", "1.0")]),
        // When .conda is filtered by timestamp, fall back to .tar.bz2
        SolverCase::new("exclude_newer prefers .tar.bz2 when .conda is too new")
            .repository([foo_tarbz2.clone(), foo_conda, foo_newer_version])
            .specs(["foo"])
            .exclude_newer("2022-01-01T00:00:00Z")
            .expect_present([foo_tarbz2]),
    ]);
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

/// Test that packages with unparsable dependencies don't crash the solver.
/// This can happen when repodata contains malformed dependency strings.
pub(super) fn solve_with_unparsable_dependency<T: SolverImpl + Default>() {
    // Create two versions of a package, one with valid deps and one with invalid deps
    let pkg_valid = PackageBuilder::new("sortme")
        .version("1.0.0")
        .build_string("build_a")
        .build();

    let pkg_invalid = PackageBuilder::new("sortme")
        .version("2.0.0")
        .build_string("build_b")
        // This is a malformed dependency string that can't be parsed as a MatchSpec
        .depends(["this-is-not-a-valid-matchspec @#$%^&*()"])
        .build();

    // The solver should handle the unparsable dependency gracefully,
    // selecting the package with valid dependencies
    SolverCase::new("solver handles unparsable dependencies gracefully")
        .repository([pkg_valid, pkg_invalid])
        .specs(["sortme"])
        .expect_present([("sortme", "1.0.0")])
        .run::<T>();
}

/// A reproducer of issue <https://github.com/prefix-dev/resolvo/issues/188>
pub(super) fn resolvo_issue_188<T: SolverImpl + Default>() {
    let pkg_a1 = PackageBuilder::new("dependency_package")
        .version("0.1.0")
        .build();
    let pkg_a2 = PackageBuilder::new("dependency_package")
        .version("0.2.0")
        .build();
    let pkg_b1 = PackageBuilder::new("dependent_package")
        .version("1.0.0")
        .depends(["dependency_package ==0.2.0"])
        .build();

    SolverCase::new("solver respects constraints on package versions")
        .repository([pkg_a1.clone(), pkg_a2.clone(), pkg_b1])
        .specs(["dependency_package <0.2.0"])
        .constraints(["dependent_package ==1.0.0"])
        // `dependent_package` was not requested and thus we expect it to be absent
        .expect_absent(["dependent_package"])
        .run::<T>();
}
