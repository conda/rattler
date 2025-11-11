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

/// Tests conditional dependencies with an unusual package name that won't conflict with real packages
pub(super) fn solve_conditional_unusual_package<T: SolverImpl + Default>() {
    use rattler_conda_types::Version;

    // Use an unusual package name to avoid conflicts with real packages
    // This uses double underscores and a very specific naming pattern that won't appear in conda-forge
    let unusual_name = "__test_conditional_never_use_pkg_xyz123";

    let python310_pkg = PackageBuilder::new("python").version("3.10.0").build();
    let python311_pkg = PackageBuilder::new("python").version("3.11.0").build();
    let python312_pkg = PackageBuilder::new("python").version("3.12.0").build();

    let dep_alpha = PackageBuilder::new("__dep_alpha").version("1.0.0").build();
    let dep_beta = PackageBuilder::new("__dep_beta").version("2.0.0").build();
    let dep_gamma = PackageBuilder::new("__dep_gamma").version("3.0.0").build();
    let dep_delta = PackageBuilder::new("__dep_delta").version("4.0.0").build();

    // Package with multiple conditional dependencies
    let multi_conditional = PackageBuilder::new(unusual_name)
        .version("1.0.0")
        .depends([
            "python",
            "__dep_alpha; if python>=3.11",
            "__dep_beta; if python<3.11",
            "__dep_gamma; if python>=3.10 and python<3.12",
            "__dep_delta; if python>=3.12 or python<3.10.5",
        ])
        .build();

    // Package with chained conditional dependencies
    let chained_conditionals = PackageBuilder::new(unusual_name)
        .version("2.0.0")
        .depends([
            "python>=3.10",
            "__dep_alpha; if __dep_beta",
            "__dep_beta; if python>=3.11",
        ])
        .build();

    // Package with negation-like behavior using OR
    let negation_conditional = PackageBuilder::new(unusual_name)
        .version("3.0.0")
        .depends([
            "python",
            "__dep_alpha; if python<3.10 or python>=3.12",
        ])
        .build();

    let unix_virtual = GenericVirtualPackage {
        name: "__unix".parse().unwrap(),
        version: Version::from_str("0").unwrap(),
        build_string: "0".to_string(),
    };

    let cuda_virtual = GenericVirtualPackage {
        name: "__cuda".parse().unwrap(),
        version: Version::from_str("11.0").unwrap(),
        build_string: "0".to_string(),
    };

    // Package with virtual package conditions
    let virtual_conditionals = PackageBuilder::new(unusual_name)
        .version("4.0.0")
        .depends([
            "__dep_alpha; if __unix",
            "__dep_beta; if __cuda>=11.0",
            "__dep_gamma; if __unix and __cuda",
        ])
        .build();

    run_solver_cases::<T>(&[
        SolverCase::new("Multiple conditionals: python=3.10 pulls alpha=false, beta=true, gamma=true, delta=true")
            .repository(vec![
                multi_conditional.clone(),
                python310_pkg.clone(),
                dep_alpha.clone(),
                dep_beta.clone(),
                dep_gamma.clone(),
                dep_delta.clone(),
            ])
            .specs([unusual_name, "python=3.10"])
            .expect_present([&multi_conditional, &python310_pkg, &dep_beta, &dep_gamma, &dep_delta])
            .expect_absent([&dep_alpha]),
        SolverCase::new("Multiple conditionals: python=3.11 pulls alpha=true, beta=false, gamma=true, delta=false")
            .repository(vec![
                multi_conditional.clone(),
                python311_pkg.clone(),
                dep_alpha.clone(),
                dep_beta.clone(),
                dep_gamma.clone(),
                dep_delta.clone(),
            ])
            .specs([unusual_name, "python=3.11"])
            .expect_present([&multi_conditional, &python311_pkg, &dep_alpha, &dep_gamma])
            .expect_absent([&dep_beta, &dep_delta]),
        SolverCase::new("Multiple conditionals: python=3.12 pulls alpha=true, beta=false, gamma=false, delta=true")
            .repository(vec![
                multi_conditional.clone(),
                python312_pkg.clone(),
                dep_alpha.clone(),
                dep_beta.clone(),
                dep_gamma.clone(),
                dep_delta.clone(),
            ])
            .specs([unusual_name, "python=3.12"])
            .expect_present([&multi_conditional, &python312_pkg, &dep_alpha, &dep_delta])
            .expect_absent([&dep_beta, &dep_gamma]),
        SolverCase::new("Chained conditionals: python=3.11 activates beta, which activates alpha")
            .repository(vec![
                chained_conditionals.clone(),
                python311_pkg.clone(),
                dep_alpha.clone(),
                dep_beta.clone(),
            ])
            .specs([unusual_name, "python=3.11"])
            .expect_present([&chained_conditionals, &python311_pkg, &dep_alpha, &dep_beta]),
        SolverCase::new("Chained conditionals: python=3.10 does not activate beta or alpha")
            .repository(vec![
                chained_conditionals.clone(),
                python310_pkg.clone(),
                dep_alpha.clone(),
                dep_beta.clone(),
            ])
            .specs([unusual_name, "python=3.10"])
            .expect_present([&chained_conditionals, &python310_pkg])
            .expect_absent([&dep_alpha, &dep_beta]),
        SolverCase::new("Negation-like: python=3.10 does not pull dep_alpha (in the middle range)")
            .repository(vec![
                negation_conditional.clone(),
                python310_pkg.clone(),
                dep_alpha.clone(),
            ])
            .specs([unusual_name, "python=3.10"])
            .expect_present([&negation_conditional, &python310_pkg])
            .expect_absent([&dep_alpha]),
        SolverCase::new("Negation-like: python=3.12 pulls dep_alpha (above the range)")
            .repository(vec![
                negation_conditional.clone(),
                python312_pkg.clone(),
                dep_alpha.clone(),
            ])
            .specs([unusual_name, "python=3.12"])
            .expect_present([&negation_conditional, &python312_pkg, &dep_alpha]),
        SolverCase::new("Virtual package conditionals: with __unix only")
            .repository(vec![
                virtual_conditionals.clone(),
                dep_alpha.clone(),
                dep_beta.clone(),
                dep_gamma.clone(),
            ])
            .specs([unusual_name])
            .virtual_packages(vec![unix_virtual.clone()])
            .expect_present([&virtual_conditionals, &dep_alpha])
            .expect_absent([&dep_beta, &dep_gamma]),
        SolverCase::new("Virtual package conditionals: with __cuda only")
            .repository(vec![
                virtual_conditionals.clone(),
                dep_alpha.clone(),
                dep_beta.clone(),
                dep_gamma.clone(),
            ])
            .specs([unusual_name])
            .virtual_packages(vec![cuda_virtual.clone()])
            .expect_present([&virtual_conditionals, &dep_beta])
            .expect_absent([&dep_alpha, &dep_gamma]),
        SolverCase::new("Virtual package conditionals: with both __unix and __cuda")
            .repository(vec![
                virtual_conditionals.clone(),
                dep_alpha.clone(),
                dep_beta.clone(),
                dep_gamma.clone(),
            ])
            .specs([unusual_name])
            .virtual_packages(vec![unix_virtual, cuda_virtual])
            .expect_present([&virtual_conditionals, &dep_alpha, &dep_beta, &dep_gamma]),
    ]);
}
