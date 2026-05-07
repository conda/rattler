//! Integration tests for minimum package age filtering.
//!
//! These tests verify that the `min_age` configuration correctly filters out
//! packages that have been published too recently, helping reduce the risk
//! of installing compromised packages.

use chrono::{DateTime, Utc};
use rattler_solve::{ExcludeNewer, SolverImpl};

use crate::helpers::{PackageBuilder, SolverCase};

fn fixed_now() -> DateTime<Utc> {
    "2026-03-24T00:00:00Z"
        .parse()
        .expect("invalid fixed test timestamp")
}

fn exclude_newer_duration_config(min_age: std::time::Duration) -> ExcludeNewer {
    ExcludeNewer::from_duration_with_now(min_age, fixed_now())
}

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

/// Test that `min_age` filters out packages that are too new.
pub fn solve_min_age_filters_new_packages<T: SolverImpl + Default>() {
    let repo = create_timestamped_repo();

    // 1000 days minimum age - this filters out 2024 packages
    // (from 2026-03-24, the cutoff is 2023-06-28)
    let min_age = std::time::Duration::from_secs(1000 * 24 * 60 * 60);

    SolverCase::new("min_age filters new packages")
        .repository(repo)
        .specs(["pkg-a"])
        .exclude_newer(exclude_newer_duration_config(min_age))
        .expect_present([("pkg-a", "1.0")])
        .expect_absent([("pkg-a", "2.0")])
        .run::<T>();
}

/// Test that packages can override the global cutoff.
pub fn solve_min_age_with_package_override<T: SolverImpl + Default>() {
    let repo = create_timestamped_repo();

    // 1000 days minimum age - would normally filter out 2024 packages
    let min_age = std::time::Duration::from_secs(1000 * 24 * 60 * 60);

    // But we override "pkg-a" to allow the newest available version
    let config = exclude_newer_duration_config(min_age).with_package_duration_with_now(
        "pkg-a".parse().unwrap(),
        std::time::Duration::ZERO,
        fixed_now(),
    );

    SolverCase::new("min_age with package override")
        .repository(repo)
        .specs(["pkg-a"])
        .exclude_newer(config)
        .expect_present([("pkg-a", "2.0")]) // Gets latest because the package cutoff overrides the global one
        .run::<T>();
}

/// Test that `min_age` applies to dependencies as well.
pub fn solve_min_age_with_dependencies<T: SolverImpl + Default>() {
    let repo = create_timestamped_repo();

    // 1000 days minimum age - filters out 2024 packages
    let min_age = std::time::Duration::from_secs(1000 * 24 * 60 * 60);

    SolverCase::new("min_age with dependencies")
        .repository(repo)
        .specs(["pkg-b"])
        .exclude_newer(exclude_newer_duration_config(min_age))
        // pkg-b 2.0 requires pkg-a >=2, but pkg-a 2.0 is too new
        // So we should get pkg-b 1.0 and pkg-a 1.0
        .expect_present([("pkg-b", "1.0"), ("pkg-a", "1.0")])
        .expect_absent([("pkg-b", "2.0"), ("pkg-a", "2.0")])
        .run::<T>();
}

/// Test that package-specific cutoffs work correctly with dependencies.
pub fn solve_min_age_package_override_dependency<T: SolverImpl + Default>() {
    let repo = create_timestamped_repo();

    // 1000 days minimum age
    let min_age = std::time::Duration::from_secs(1000 * 24 * 60 * 60);

    // Override pkg-a but not pkg-b
    let config = exclude_newer_duration_config(min_age).with_package_duration_with_now(
        "pkg-a".parse().unwrap(),
        std::time::Duration::ZERO,
        fixed_now(),
    );

    SolverCase::new("min_age package override dependency")
        .repository(repo)
        .specs(["pkg-b"])
        .exclude_newer(config)
        // pkg-b 2.0 is too new and not overridden, so we get pkg-b 1.0
        // pkg-a is overridden, but pkg-b 1.0 only requires "pkg-a" (any version)
        // so the solver can choose either pkg-a 1.0 or 2.0
        .expect_present(["pkg-b"])
        .expect_absent([("pkg-b", "2.0")])
        .run::<T>();
}

/// Test that packages without timestamps are excluded by default.
pub fn solve_min_age_excludes_unknown_timestamp<T: SolverImpl + Default>() {
    let repo = vec![
        PackageBuilder::new("pkg-a")
            .version("1.0")
            .timestamp("2020-01-15T12:00:00Z")
            .build(),
        PackageBuilder::new("pkg-a")
            .version("2.0")
            // No timestamp - should be excluded by default
            .build(),
    ];

    // 1000 days minimum age
    let min_age = std::time::Duration::from_secs(1000 * 24 * 60 * 60);

    SolverCase::new("min_age excludes unknown timestamp by default")
        .repository(repo)
        .specs(["pkg-a"])
        .exclude_newer(exclude_newer_duration_config(min_age))
        // pkg-a 2.0 has no timestamp and should be excluded
        // pkg-a 1.0 has an old timestamp and should be selected
        .expect_present([("pkg-a", "1.0")])
        .expect_absent([("pkg-a", "2.0")])
        .run::<T>();
}

/// Test that package-specific cutoffs do not override missing timestamps.
pub fn solve_min_age_package_override_no_timestamp<T: SolverImpl + Default>() {
    let repo = vec![
        PackageBuilder::new("pkg-a")
            .version("1.0")
            .timestamp("2020-01-15T12:00:00Z")
            .build(),
        PackageBuilder::new("pkg-a")
            .version("2.0")
            // No timestamp - would normally be excluded
            .build(),
    ];

    // 1000 days minimum age
    let min_age = std::time::Duration::from_secs(1000 * 24 * 60 * 60);

    // Override pkg-a to allow the newest timestamps, but unknown timestamps
    // still require include_unknown_timestamp=true.
    let config = exclude_newer_duration_config(min_age).with_package_duration_with_now(
        "pkg-a".parse().unwrap(),
        std::time::Duration::ZERO,
        fixed_now(),
    );

    SolverCase::new("min_age package override without timestamp")
        .repository(repo)
        .specs(["pkg-a"])
        .exclude_newer(config)
        .expect_present([("pkg-a", "1.0")])
        .expect_absent([("pkg-a", "2.0")])
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
        .exclude_newer(exclude_newer_duration_config(min_age).with_include_unknown_timestamp(true))
        // Both packages should be available:
        // - pkg-no-ts has no timestamp but we explicitly include unknown timestamps
        // - pkg-old has an old timestamp so it passes the filter
        .expect_present(["pkg-no-ts", "pkg-old"])
        .run::<T>();
}

/// Test that channel-specific minimum ages override the global minimum age.
pub fn solve_min_age_per_channel<T: SolverImpl + Default>() {
    let repo = vec![
        PackageBuilder::new("pkg-a")
            .version("1.0")
            .channel("stable")
            .timestamp("2020-01-15T12:00:00Z")
            .build(),
        PackageBuilder::new("pkg-a")
            .version("2.0")
            .channel("stable")
            .timestamp("2024-06-15T12:00:00Z")
            .build(),
        PackageBuilder::new("pkg-b")
            .version("1.0")
            .channel("internal")
            .timestamp("2020-01-15T12:00:00Z")
            .build(),
        PackageBuilder::new("pkg-b")
            .version("2.0")
            .channel("internal")
            .timestamp("2024-06-15T12:00:00Z")
            .build(),
    ];

    let min_age = std::time::Duration::from_secs(1000 * 24 * 60 * 60);
    let config = exclude_newer_duration_config(min_age).with_channel_duration_with_now(
        "internal",
        std::time::Duration::ZERO,
        fixed_now(),
    );

    SolverCase::new("min_age per channel")
        .repository(repo)
        .specs(["pkg-a", "pkg-b"])
        .exclude_newer(config)
        .expect_present([("pkg-a", "1.0"), ("pkg-b", "2.0")])
        .expect_absent([("pkg-a", "2.0"), ("pkg-b", "1.0")])
        .run::<T>();
}
