//! Tests for extras (optional dependencies) support

use super::helpers::{PackageBuilder, SolverCase, run_solver_cases};
use rattler_solve::SolverImpl;

/// Test that extras pull in the correct optional dependencies
pub(super) fn solve_extras_basic<T: SolverImpl + Default>() {
    let bar_pkg = PackageBuilder::new("bar")
        .version("1.0.0")
        .build();

    let foo_pkg = PackageBuilder::new("foo")
        .version("1.0.0")
        .extra_depends("with-bar", ["bar <2"])
        .build();

    run_solver_cases::<T>(&[
        SolverCase::new("Extras pull in optional dependencies")
            .repository(vec![foo_pkg.clone(), bar_pkg.clone()])
            .specs(["foo[extras=[with-bar]]"])
            .expect_present([&foo_pkg, &bar_pkg]),
        SolverCase::new("Without extras, optional dependencies are not included")
            .repository(vec![foo_pkg.clone(), bar_pkg.clone()])
            .specs(["foo"])
            .expect_present([&foo_pkg])
            .expect_absent([&bar_pkg]),
    ]);
}

/// Test that extras influence version selection of dependencies
pub(super) fn solve_extras_version_restriction<T: SolverImpl + Default>() {
    let bar_v1 = PackageBuilder::new("bar")
        .version("1.0.0")
        .build();

    let bar_v2 = PackageBuilder::new("bar")
        .version("2.0.0")
        .build();

    let foo_pkg = PackageBuilder::new("foo")
        .version("1.0.0")
        .extra_depends("with-bar", ["bar <2"])
        .build();

    run_solver_cases::<T>(&[
        SolverCase::new("Extra restricts bar to version 1")
            .repository(vec![foo_pkg.clone(), bar_v1.clone(), bar_v2.clone()])
            .specs(["foo[extras=[with-bar]]", "bar"])
            .expect_present([&foo_pkg, &bar_v1])
            .expect_absent([&bar_v2]),
    ]);
}

/// Test multiple extras on the same package
pub(super) fn solve_multiple_extras<T: SolverImpl + Default>() {
    let dep1 = PackageBuilder::new("dep1").version("1.0.0").build();
    let dep2 = PackageBuilder::new("dep2").version("1.0.0").build();

    let pkg = PackageBuilder::new("pkg")
        .version("1.0.0")
        .extra_depends("extra1", ["dep1"])
        .extra_depends("extra2", ["dep2"])
        .build();

    run_solver_cases::<T>(&[
        SolverCase::new("Single extra pulls only its dependencies")
            .repository(vec![pkg.clone(), dep1.clone(), dep2.clone()])
            .specs(["pkg[extras=[extra1]]"])
            .expect_present([&pkg, &dep1])
            .expect_absent([&dep2]),
        SolverCase::new("Multiple extras pull all their dependencies")
            .repository(vec![pkg.clone(), dep1.clone(), dep2.clone()])
            .specs(["pkg[extras=[extra1,extra2]]"])
            .expect_present([&pkg, &dep1, &dep2]),
        SolverCase::new("No extras pull no optional dependencies")
            .repository(vec![pkg.clone(), dep1.clone(), dep2.clone()])
            .specs(["pkg"])
            .expect_present([&pkg])
            .expect_absent([&dep1, &dep2]),
    ]);
}

/// Test extras with complex dependency constraints
pub(super) fn solve_extras_complex_constraints<T: SolverImpl + Default>() {
    let python38 = PackageBuilder::new("python").version("3.8.0").build();
    let python39 = PackageBuilder::new("python").version("3.9.0").build();
    let python310 = PackageBuilder::new("python").version("3.10.0").build();

    let numpy_v1 = PackageBuilder::new("numpy").version("1.20.0").build();
    let numpy_v2 = PackageBuilder::new("numpy").version("1.24.0").build();

    let pkg = PackageBuilder::new("scientific-pkg")
        .version("1.0.0")
        .depends(["python"])
        .extra_depends("numpy", ["numpy >=1.20,<1.24", "python >=3.8"])
        .build();

    run_solver_cases::<T>(&[
        SolverCase::new("Extra with multiple constraints selects correct versions")
            .repository(vec![
                pkg.clone(),
                python38.clone(),
                python39.clone(),
                python310.clone(),
                numpy_v1.clone(),
                numpy_v2.clone(),
            ])
            .specs(["scientific-pkg[extras=[numpy]]", "python=3.9"])
            .expect_present([&pkg, &python39, &numpy_v1])
            .expect_absent([&numpy_v2]),
    ]);
}
