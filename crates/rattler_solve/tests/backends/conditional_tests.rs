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

/// Test for https://github.com/conda/rattler/issues/1917
/// Solver fails with platform-specific conditional dependencies.
pub(super) fn rattler_issue_1917_platform_conditionals<T: SolverImpl + Default>() {
    use rattler_conda_types::Version;

    // Platform-specific conditional dependencies
    // Package declares platform-specific deps that should resolve when virtual package is present
    let platform_pkg = PackageBuilder::new("package")
        .version("1.0.0")
        .depends([
            "osx-dependency; if __osx",
            "linux-dependency; if __linux",
            "win-dependency; if __win",
        ])
        .build();

    let osx_dep = PackageBuilder::new("osx-dependency")
        .version("1.0.0")
        .build();
    let linux_dep = PackageBuilder::new("linux-dependency")
        .version("1.0.0")
        .build();
    let win_dep = PackageBuilder::new("win-dependency")
        .version("1.0.0")
        .build();

    let osx_virtual = GenericVirtualPackage {
        name: "__osx".parse().unwrap(),
        version: Version::from_str("15.6.1").unwrap(),
        build_string: "0".to_string(),
    };

    let linux_virtual = GenericVirtualPackage {
        name: "__linux".parse().unwrap(),
        version: Version::from_str("0").unwrap(),
        build_string: "0".to_string(),
    };

    let win_virtual = GenericVirtualPackage {
        name: "__win".parse().unwrap(),
        version: Version::from_str("0").unwrap(),
        build_string: "0".to_string(),
    };

    run_solver_cases::<T>(&[
        SolverCase::new("platform conditional: osx dependency resolved when __osx present")
            .repository(vec![
                platform_pkg.clone(),
                osx_dep.clone(),
                linux_dep.clone(),
                win_dep.clone(),
            ])
            .specs(["package"])
            .virtual_packages(vec![osx_virtual.clone()])
            .expect_present([&platform_pkg, &osx_dep])
            .expect_absent([&linux_dep, &win_dep]),
        SolverCase::new("platform conditional: linux dependency resolved when __linux present")
            .repository(vec![
                platform_pkg.clone(),
                osx_dep.clone(),
                linux_dep.clone(),
                win_dep.clone(),
            ])
            .specs(["package"])
            .virtual_packages(vec![linux_virtual])
            .expect_present([&platform_pkg, &linux_dep])
            .expect_absent([&osx_dep, &win_dep]),
        SolverCase::new("platform conditional: win dependency resolved when __win present")
            .repository(vec![
                platform_pkg.clone(),
                osx_dep.clone(),
                linux_dep.clone(),
                win_dep.clone(),
            ])
            .specs(["package"])
            .virtual_packages(vec![win_virtual])
            .expect_present([&platform_pkg, &win_dep])
            .expect_absent([&osx_dep, &linux_dep]),
    ]);
}

/// Test for https://github.com/conda/rattler/issues/1917
pub(super) fn rattler_issue_1917_version_conditionals<T: SolverImpl + Default>() {
    use rattler_conda_types::Version;

    // conditional-dependency declares: "package; if side-dependency=0.2"
    // package itself has platform-conditional dependencies
    let conditional_dep_pkg = PackageBuilder::new("conditional-dependency")
        .version("1.0.0")
        .depends(["package; if side-dependency=0.2"])
        .build();

    // package has its own conditional dependencies (chained conditionals)
    let package = PackageBuilder::new("package")
        .version("1.0.0")
        .depends([
            "osx-dependency; if __osx",
            "linux-dependency; if __linux",
            "win-dependency; if __win",
        ])
        .build();

    let osx_dep = PackageBuilder::new("osx-dependency")
        .version("1.0.0")
        .build();
    let linux_dep = PackageBuilder::new("linux-dependency")
        .version("1.0.0")
        .build();
    let win_dep = PackageBuilder::new("win-dependency")
        .version("1.0.0")
        .build();
    let side_dep_v01 = PackageBuilder::new("side-dependency")
        .version("0.1.0")
        .build();
    let side_dep_v02 = PackageBuilder::new("side-dependency")
        .version("0.2.0")
        .build();

    let osx_virtual = GenericVirtualPackage {
        name: "__osx".parse().unwrap(),
        version: Version::from_str("15.6.1").unwrap(),
        build_string: "0".to_string(),
    };

    run_solver_cases::<T>(&[
        SolverCase::new("chained conditionals: package with nested conditional deps included when side-dependency=0.2")
            .repository(vec![
                conditional_dep_pkg.clone(),
                package.clone(),
                osx_dep.clone(),
                linux_dep.clone(),
                win_dep.clone(),
                side_dep_v01.clone(),
                side_dep_v02.clone(),
            ])
            .specs(["conditional-dependency", "side-dependency=0.2"])
            .virtual_packages(vec![osx_virtual.clone()])
            .expect_present([&conditional_dep_pkg, &package, &osx_dep, &side_dep_v02])
            .expect_absent([&side_dep_v01, &linux_dep, &win_dep]),
        SolverCase::new("chained conditionals: package excluded when side-dependency=0.1")
            .repository(vec![
                conditional_dep_pkg.clone(),
                package.clone(),
                osx_dep.clone(),
                linux_dep.clone(),
                win_dep.clone(),
                side_dep_v01.clone(),
                side_dep_v02.clone(),
            ])
            .specs(["conditional-dependency", "side-dependency=0.1"])
            .virtual_packages(vec![osx_virtual])
            .expect_present([&conditional_dep_pkg, &side_dep_v01])
            .expect_absent([&package, &osx_dep, &linux_dep, &win_dep, &side_dep_v02]),
    ]);
}
