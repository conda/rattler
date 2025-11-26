use super::helpers::package_builder::PackageBuilder;
use super::helpers::solver_case::{run_solver_cases, SolverCase};
use super::*;

pub(super) fn solve_conditional_dependencies<T: SolverImpl + Default>() {
    use rattler_conda_types::Version;

    let conditional_pkg = PackageBuilder::new("conditional-pkg")
        .version("1.0.0")
        .depends(["python >=3.8", "numpy; if python >=3.9", "scipy; if __unix"])
        .build();

    let python39_pkg = PackageBuilder::new("python").version("3.9.0").build();
    let python38_pkg = PackageBuilder::new("python").version("3.8.0").build();
    let numpy_pkg = PackageBuilder::new("numpy").version("1.21.0").build();
    let scipy_pkg = PackageBuilder::new("scipy").version("1.7.0").build();

    let unix_virtual = GenericVirtualPackage {
        name: "__unix".parse().unwrap(),
        version: Version::from_str("0").unwrap(),
        build_string: "0".to_string(),
    };

    run_solver_cases::<T>(&[
        SolverCase::new("python 3.9 pulls conditional deps")
            .repository(vec![
                conditional_pkg.clone(),
                python39_pkg.clone(),
                numpy_pkg.clone(),
                scipy_pkg.clone(),
            ])
            .specs(["conditional-pkg", "python=3.9"])
            .virtual_packages(vec![unix_virtual.clone()])
            .expect_present([&conditional_pkg, &python39_pkg, &numpy_pkg, &scipy_pkg]),
        SolverCase::new("python 3.8 skips numpy")
            .repository(vec![
                conditional_pkg.clone(),
                python38_pkg.clone(),
                numpy_pkg.clone(),
                scipy_pkg.clone(),
            ])
            .specs(["conditional-pkg", "python=3.8"])
            .virtual_packages(vec![unix_virtual])
            .expect_present([&conditional_pkg, &python38_pkg, &scipy_pkg])
            .expect_absent([&numpy_pkg]),
    ]);
}

pub(super) fn solve_complex_conditional_dependencies<T: SolverImpl + Default>() {
    use rattler_conda_types::Version;

    let python39_pkg = PackageBuilder::new("python").version("3.9.0").build();
    let python38_pkg = PackageBuilder::new("python").version("3.8.0").build();
    let python36_pkg = PackageBuilder::new("python").version("3.6.0").build();

    let pkg_a = PackageBuilder::new("pkg-a").build();
    let pkg_b = PackageBuilder::new("pkg-b").build();
    let pkg_c = PackageBuilder::new("pkg-c").build();

    let base_complex_pkg = PackageBuilder::new("complex-pkg");

    let complex_pkg_and = base_complex_pkg
        .clone()
        .depends(["python", "pkg-a; if python>=3.8 and python<3.9.5"])
        .build();

    let complex_pkg_or = base_complex_pkg
        .clone()
        .depends(["python", "pkg-b; if python>=3.9 or python<3.7"])
        .build();

    let complex_pkg_nested = base_complex_pkg
        .depends(["python", "pkg-c; if (python>=3.9 or python<3.7) and __unix"])
        .build();

    let unix_virtual = GenericVirtualPackage {
        name: "__unix".parse().unwrap(),
        version: Version::from_str("0").unwrap(),
        build_string: "0".to_string(),
    };

    run_solver_cases::<T>(&[
        SolverCase::new("AND condition satisfied with python=3.8")
            .repository(vec![
                complex_pkg_and.clone(),
                python38_pkg.clone(),
                pkg_a.clone(),
            ])
            .specs(["complex-pkg", "python=3.8"])
            .expect_present([&pkg_a]),
        SolverCase::new("AND condition fails with python=3.6")
            .repository(vec![
                complex_pkg_and.clone(),
                python36_pkg.clone(),
                pkg_a.clone(),
            ])
            .specs(["complex-pkg", "python=3.6"])
            .expect_absent([&pkg_a]),
        SolverCase::new("OR condition satisfied with python=3.9")
            .repository(vec![
                complex_pkg_or.clone(),
                python39_pkg.clone(),
                pkg_b.clone(),
            ])
            .specs(["complex-pkg", "python=3.9"])
            .expect_present([&pkg_b]),
        SolverCase::new("OR condition satisfied with python=3.6")
            .repository(vec![
                complex_pkg_or.clone(),
                python36_pkg.clone(),
                pkg_b.clone(),
            ])
            .specs(["complex-pkg", "python=3.6"])
            .expect_present([&pkg_b]),
        SolverCase::new("OR condition fails with python=3.8")
            .repository(vec![
                complex_pkg_or.clone(),
                python38_pkg.clone(),
                pkg_b.clone(),
            ])
            .specs(["complex-pkg", "python=3.8"])
            .expect_absent([&pkg_b]),
        SolverCase::new("Nested condition satisfied with __unix and python=3.9")
            .repository(vec![
                complex_pkg_nested.clone(),
                python39_pkg.clone(),
                pkg_c.clone(),
            ])
            .specs(["complex-pkg", "python=3.9"])
            .virtual_packages(vec![unix_virtual.clone()])
            .expect_present([&pkg_c]),
        SolverCase::new("Nested condition fails with python=3.8 even with __unix")
            .repository(vec![
                complex_pkg_nested.clone(),
                python38_pkg.clone(),
                pkg_c.clone(),
            ])
            .specs(["complex-pkg", "python=3.8"])
            .virtual_packages(vec![unix_virtual])
            .expect_absent([&pkg_c]),
    ]);
}

/// Test that conditional root requirements work when condition is satisfied.
pub(super) fn solve_conditional_root_requirement_satisfied<T: SolverImpl + Default>() {
    use rattler_conda_types::Version;

    let python = PackageBuilder::new("python").version("3.9.0").build();

    let unix_virtual = GenericVirtualPackage {
        name: "__unix".parse().unwrap(),
        version: Version::from_str("0").unwrap(),
        build_string: "0".to_string(),
    };

    SolverCase::new("conditional root spec includes package when condition satisfied")
        .repository([python.clone()])
        .specs(["python; if __unix"])
        .virtual_packages(vec![unix_virtual])
        .expect_present([("python", "3.9.0")])
        .run::<T>();
}

/// Test that conditional root requirements work when condition is NOT satisfied.
pub(super) fn solve_conditional_root_requirement_not_satisfied<T: SolverImpl + Default>() {
    use rattler_conda_types::Version;

    let python = PackageBuilder::new("python").version("3.9.0").build();

    // Only __unix is present, but spec requires __win
    let unix_virtual = GenericVirtualPackage {
        name: "__unix".parse().unwrap(),
        version: Version::from_str("0").unwrap(),
        build_string: "0".to_string(),
    };

    SolverCase::new("conditional root spec excludes package when condition not satisfied")
        .repository([python.clone()])
        .specs(["python; if __win"])
        .virtual_packages(vec![unix_virtual])
        .expect_absent(["python"])
        .run::<T>();
}

/// Test that conditional root requirements with AND logic work correctly.
pub(super) fn solve_conditional_root_requirement_with_logic<T: SolverImpl + Default>() {
    use rattler_conda_types::Version;

    let python = PackageBuilder::new("python").version("3.9.0").build();

    let unix_virtual = GenericVirtualPackage {
        name: "__unix".parse().unwrap(),
        version: Version::from_str("0").unwrap(),
        build_string: "0".to_string(),
    };
    let linux_virtual = GenericVirtualPackage {
        name: "__linux".parse().unwrap(),
        version: Version::from_str("0").unwrap(),
        build_string: "0".to_string(),
    };

    SolverCase::new(
        "conditional root spec with AND logic includes package when both conditions satisfied",
    )
    .repository([python.clone()])
    .specs(["python; if __unix and __linux"])
    .virtual_packages(vec![unix_virtual, linux_virtual])
    .expect_present([("python", "3.9.0")])
    .run::<T>();
}
