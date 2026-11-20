//! Tests for extras (optional dependencies) support

use super::helpers::{PackageBuilder, SolverCase, run_solver_cases};
use rattler_conda_types::{MatchSpec, ParseMatchSpecOptions, RepoDataRecord};
use rattler_solve::{SolveError, SolverImpl, SolverTask};

/// Test that extras pull in the correct optional dependencies
pub(super) fn solve_extras_basic<T: SolverImpl + Default>() {
    let bar_pkg = PackageBuilder::new("bar").version("1.0.0").build();

    let foo_pkg = PackageBuilder::new("foo")
        .version("1.0.0")
        .extra_depends("with-bar", ["bar <2"])
        .build();

    run_solver_cases::<T>(&[
        SolverCase::new("Extras pull in optional dependencies")
            .repository(vec![foo_pkg.clone(), bar_pkg.clone()])
            .specs(["foo[extras=[with-bar]]"])
            .expect_present([&foo_pkg, &bar_pkg])
            .expect_extras([("foo", ["with-bar"])]),
        SolverCase::new("Without extras, optional dependencies are not included")
            .repository(vec![foo_pkg.clone(), bar_pkg.clone()])
            .specs(["foo"])
            .expect_present([&foo_pkg])
            .expect_absent([&bar_pkg]),
    ]);
}

/// Test that extras influence version selection of dependencies
pub(super) fn solve_extras_version_restriction<T: SolverImpl + Default>() {
    let bar_v1 = PackageBuilder::new("bar").version("1.0.0").build();

    let bar_v2 = PackageBuilder::new("bar").version("2.0.0").build();

    let foo_pkg = PackageBuilder::new("foo")
        .version("1.0.0")
        .extra_depends("with-bar", ["bar <2"])
        .build();

    run_solver_cases::<T>(&[SolverCase::new("Extra restricts bar to version 1")
        .repository(vec![foo_pkg.clone(), bar_v1.clone(), bar_v2.clone()])
        .specs(["foo[extras=[with-bar]]", "bar"])
        .expect_present([&foo_pkg, &bar_v1])
        .expect_absent([&bar_v2])]);
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
            .expect_present([&pkg, &dep1, &dep2])
            .expect_extras([("pkg", ["extra1", "extra2"])]),
        SolverCase::new("No extras pull no optional dependencies")
            .repository(vec![pkg.clone(), dep1.clone(), dep2.clone()])
            .specs(["pkg"])
            .expect_present([&pkg])
            .expect_absent([&dep1, &dep2]),
    ]);
}

/// Only records that declare an extra in `extra_depends` provide it, so asking
/// for an extra must not select a version that does not declare it — not even
/// when that version sorts higher.
pub(super) fn solve_extras_select_version_providing_extra<T: SolverImpl + Default>() {
    let bar_pkg = PackageBuilder::new("bar").version("1.0.0").build();

    // The newest version dropped the extra, the older one still declares it.
    let foo_with_extra = PackageBuilder::new("foo")
        .version("1.0.0")
        .extra_depends("json", ["bar"])
        .build();
    let foo_without_extra = PackageBuilder::new("foo").version("2.0.0").build();

    run_solver_cases::<T>(&[
        SolverCase::new("Only the version that declares the extra can satisfy it")
            .repository(vec![
                foo_with_extra.clone(),
                foo_without_extra.clone(),
                bar_pkg.clone(),
            ])
            .specs(["foo[extras=[json]]"])
            .expect_present([&foo_with_extra, &bar_pkg])
            .expect_absent([&foo_without_extra])
            .expect_extras([("foo", ["json"])]),
        SolverCase::new("Without the extra the newest version is selected")
            .repository(vec![
                foo_with_extra.clone(),
                foo_without_extra.clone(),
                bar_pkg.clone(),
            ])
            .specs(["foo"])
            .expect_present([&foo_without_extra])
            .expect_absent([&foo_with_extra, &bar_pkg]),
    ]);
}

/// An extra whose dependencies cannot be satisfied must fail the solve instead
/// of silently falling back to a version that does not declare the extra.
///
/// Regression test for `diffle[extras=[json]]`, where the extra of the newest
/// version pulled in a package that conflicted with the rest of the solution
/// and the solver quietly resolved an older `diffle` without the extra.
pub(super) fn solve_extras_unsatisfiable_extra_is_not_dropped<T: SolverImpl + Default>() {
    let bar_v1 = PackageBuilder::new("bar").version("1.0.0").build();

    // Only the newest version declares the extra, but its dependency cannot be
    // satisfied because `bar 2.0.0` does not exist.
    let foo_with_extra = PackageBuilder::new("foo")
        .version("2.0.0")
        .extra_depends("json", ["bar >=2"])
        .build();
    let foo_without_extra = PackageBuilder::new("foo").version("1.0.0").build();

    expect_unsolvable::<T>(
        vec![
            foo_with_extra.clone(),
            foo_without_extra.clone(),
            bar_v1.clone(),
        ],
        &["foo[extras=[json]]"],
    );

    // Without the extra the very same repository resolves fine.
    run_solver_cases::<T>(&[SolverCase::new("The extra is what makes the solve fail")
        .repository(vec![foo_with_extra, foo_without_extra.clone(), bar_v1])
        .specs(["foo ==1.0.0"])
        .expect_present([&foo_without_extra])]);
}

/// Requesting an extra that no version of the package declares is an error
/// rather than a no-op.
pub(super) fn solve_extras_unknown_extra_is_an_error<T: SolverImpl + Default>() {
    let foo_pkg = PackageBuilder::new("foo")
        .version("1.0.0")
        .extra_depends("json", ["bar"])
        .build();
    let bar_pkg = PackageBuilder::new("bar").version("1.0.0").build();

    expect_unsolvable::<T>(vec![foo_pkg, bar_pkg], &["foo[extras=[typo]]"]);
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

    run_solver_cases::<T>(&[SolverCase::new(
        "Extra with multiple constraints selects correct versions",
    )
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
    .expect_absent([&numpy_v2])]);
}

/// Solves `specs` against a single repository, panicking if the solve succeeds.
fn expect_unsolvable<T: SolverImpl + Default>(repo: Vec<RepoDataRecord>, specs: &[&str]) {
    let task = SolverTask {
        specs: specs
            .iter()
            .map(|spec| {
                MatchSpec::from_str(spec, ParseMatchSpecOptions::lenient().with_extras(true))
                    .unwrap()
            })
            .collect(),
        ..SolverTask::from_iter([&repo])
    };

    match T::default().solve(task) {
        Err(SolveError::Unsolvable(_)) => {}
        Err(err) => panic!("expected {specs:?} to be unsolvable, got a different error: {err}"),
        Ok(solution) => {
            let records = solution
                .records
                .iter()
                .map(|record| {
                    format!(
                        "{}={}",
                        record.package_record.name.as_normalized(),
                        record.package_record.version
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            panic!("expected {specs:?} to be unsolvable, but it resolved to: {records}");
        }
    }
}
