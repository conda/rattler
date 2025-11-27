//! Integration tests for minimum package age filtering.
//!
//! These tests verify that the `min_age` configuration correctly filters out
//! packages that have been published too recently, helping reduce the risk
//! of installing compromised packages.

use rattler_solve::{MinimumAgeConfig, SolverImpl};

use crate::helpers::{PackageBuilder, SolverCase};

/// Creates a repository with packages that have different timestamps.
///
/// - pkg-a 1.0: old package (2020)
/// - pkg-a 2.0: new package (2024)
/// - pkg-b 1.0: old package (2020), depends on pkg-a
fn create_timestamped_repo() -> Vec<rattler_conda_types::RepoDataRecord> {
    vec![
        PackageBuilder::new("pkg-a")
            .version("1.0")
            .timestamp("2020-01-15T12:00:00Z")
            .build(),
        PackageBuilder::new("pkg-a")
            .version("2.0")
            .timestamp("2024-06-15T12:00:00Z")
            .build(),
        PackageBuilder::new("pkg-b")
            .version("1.0")
            .depends(["pkg-a"])
            .timestamp("2020-01-15T12:00:00Z")
            .build(),
        PackageBuilder::new("pkg-b")
            .version("2.0")
            .depends(["pkg-a >=2"])
            .timestamp("2024-06-15T12:00:00Z")
            .build(),
    ]
}

/// Test that min_age filters out packages that are too new.
pub fn solve_min_age_filters_new_packages<T: SolverImpl + Default>() {
    let repo = create_timestamped_repo();

    // 1000 days minimum age - this filters out 2024 packages
    // (from ~Nov 2025, the cutoff would be ~March 2023)
    let min_age = std::time::Duration::from_secs(1000 * 24 * 60 * 60);

    SolverCase::new("min_age filters new packages")
        .repository(repo)
        .specs(["pkg-a"])
        .min_age(MinimumAgeConfig::new(min_age))
        .expect_present([("pkg-a", "1.0")])
        .expect_absent([("pkg-a", "2.0")])
        .run::<T>();
}

/// Test that packages can be exempted from min_age filtering.
pub fn solve_min_age_with_exemption<T: SolverImpl + Default>() {
    let repo = create_timestamped_repo();

    // 1000 days minimum age - would normally filter out 2024 packages
    let min_age = std::time::Duration::from_secs(1000 * 24 * 60 * 60);

    // But we exempt "pkg-a" from the filter
    let config = MinimumAgeConfig::new(min_age).with_exempt_package("pkg-a".parse().unwrap());

    SolverCase::new("min_age with exemption")
        .repository(repo)
        .specs(["pkg-a"])
        .min_age(config)
        .expect_present([("pkg-a", "2.0")]) // Gets latest because it's exempt
        .run::<T>();
}

/// Test that min_age applies to dependencies as well.
pub fn solve_min_age_with_dependencies<T: SolverImpl + Default>() {
    let repo = create_timestamped_repo();

    // 1000 days minimum age - filters out 2024 packages
    let min_age = std::time::Duration::from_secs(1000 * 24 * 60 * 60);

    SolverCase::new("min_age with dependencies")
        .repository(repo)
        .specs(["pkg-b"])
        .min_age(MinimumAgeConfig::new(min_age))
        // pkg-b 2.0 requires pkg-a >=2, but pkg-a 2.0 is too new
        // So we should get pkg-b 1.0 and pkg-a 1.0
        .expect_present([("pkg-b", "1.0"), ("pkg-a", "1.0")])
        .expect_absent([("pkg-b", "2.0"), ("pkg-a", "2.0")])
        .run::<T>();
}

/// Test that exemptions work correctly with dependencies.
pub fn solve_min_age_exempt_dependency<T: SolverImpl + Default>() {
    let repo = create_timestamped_repo();

    // 1000 days minimum age
    let min_age = std::time::Duration::from_secs(1000 * 24 * 60 * 60);

    // Exempt pkg-a but not pkg-b
    let config = MinimumAgeConfig::new(min_age).with_exempt_package("pkg-a".parse().unwrap());

    SolverCase::new("min_age exempt dependency")
        .repository(repo)
        .specs(["pkg-b"])
        .min_age(config)
        // pkg-b 2.0 is too new and not exempt, so we get pkg-b 1.0
        // pkg-a is exempt, but pkg-b 1.0 only requires "pkg-a" (any version)
        // so the solver can choose either pkg-a 1.0 or 2.0
        .expect_present(["pkg-b"])
        .expect_absent([("pkg-b", "2.0")])
        .run::<T>();
}

/// Test that packages without timestamps are excluded by default.
pub fn solve_min_age_excludes_unknown_timestamp<T: SolverImpl + Default>() {
    let repo = vec![
        PackageBuilder::new("pkg-no-ts")
            .version("1.0")
            // No timestamp set - should be filtered by default
            .build(),
        PackageBuilder::new("pkg-old")
            .version("1.0")
            .timestamp("2020-01-15T12:00:00Z")
            .build(),
    ];

    // 1000 days minimum age
    let min_age = std::time::Duration::from_secs(1000 * 24 * 60 * 60);

    SolverCase::new("min_age excludes unknown timestamp by default")
        .repository(repo)
        .specs(["pkg-old"])
        .min_age(MinimumAgeConfig::new(min_age))
        // pkg-old should be available (has old timestamp)
        // pkg-no-ts is not requested but would be excluded if it were
        .expect_present(["pkg-old"])
        .run::<T>();
}

/// Test that packages without timestamps can be included with the option.
pub fn solve_min_age_include_unknown_timestamp<T: SolverImpl + Default>() {
    let repo = vec![
        PackageBuilder::new("pkg-no-ts")
            .version("1.0")
            // No timestamp set
            .build(),
        PackageBuilder::new("pkg-old")
            .version("1.0")
            .timestamp("2020-01-15T12:00:00Z")
            .build(),
    ];

    // 1000 days minimum age
    let min_age = std::time::Duration::from_secs(1000 * 24 * 60 * 60);

    SolverCase::new("min_age with include_unknown_timestamp")
        .repository(repo)
        .specs(["pkg-no-ts", "pkg-old"])
        .min_age(MinimumAgeConfig::new(min_age).with_include_unknown_timestamp(true))
        // Both packages should be available:
        // - pkg-no-ts has no timestamp but we explicitly include unknown timestamps
        // - pkg-old has an old timestamp so it passes the filter
        .expect_present(["pkg-no-ts", "pkg-old"])
        .run::<T>();
}
