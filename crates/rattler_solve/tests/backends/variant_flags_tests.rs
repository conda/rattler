//! Tests for V3 package variant flags.

use super::helpers::{PackageBuilder, SolverCase, run_solver_cases};
use rattler_conda_types::Flag;
#[cfg(feature = "libsolv_c")]
use rattler_conda_types::{MatchSpec, MatchSpecCondition, ParseMatchSpecOptions, RepodataRevision};
use rattler_solve::{FlagPreference, SolverImpl};
#[cfg(feature = "libsolv_c")]
use rattler_solve::{SolveError, SolverTask};

pub(super) fn solve_flags_select_matching_variant<T: SolverImpl + Default>() {
    let cpu_pkg = PackageBuilder::new("torch")
        .version("1.0.0")
        .build_string("cpu_0")
        .flags(["cpu", "blas:openblas"])
        .build();
    let cuda_pkg = PackageBuilder::new("torch")
        .version("1.0.0")
        .build_string("cuda_0")
        .flags(["cuda", "blas:mkl"])
        .build();

    run_solver_cases::<T>(&[
        SolverCase::new("flags select exact and globbed variant")
            .repository(vec![cpu_pkg.clone(), cuda_pkg.clone()])
            .specs(["torch[flags=[cuda, blas:*]]"])
            .expect_present([&cuda_pkg])
            .expect_absent([&cpu_pkg]),
        SolverCase::new("flags can select another variant")
            .repository(vec![cpu_pkg.clone(), cuda_pkg.clone()])
            .specs(["torch[flags=[blas:openblas]]"])
            .expect_present([&cpu_pkg])
            .expect_absent([&cuda_pkg]),
    ]);
}

pub(super) fn solve_dependency_flags_select_matching_variant<T: SolverImpl + Default>() {
    let openblas = PackageBuilder::new("blas-provider")
        .version("1.0.0")
        .build_string("openblas_0")
        .flags(["blas:openblas"])
        .build();
    let mkl = PackageBuilder::new("blas-provider")
        .version("1.0.0")
        .build_string("mkl_0")
        .flags(["blas:mkl"])
        .build();
    let consumer = PackageBuilder::new("consumer")
        .version("1.0.0")
        .depends(["blas-provider[flags=[blas:mkl]]"])
        .build();

    run_solver_cases::<T>(
        &[SolverCase::new("dependency flags select matching provider")
            .repository(vec![consumer.clone(), openblas.clone(), mkl.clone()])
            .specs(["consumer"])
            .expect_present([&consumer, &mkl])
            .expect_absent([&openblas])],
    );
}

fn prefer(flag: &str) -> FlagPreference {
    FlagPreference::Prefer(Flag::try_from(flag).unwrap())
}

fn avoid(flag: &str) -> FlagPreference {
    FlagPreference::Avoid(Flag::try_from(flag).unwrap())
}

pub(super) fn solve_flag_preferences_order_variants<T: SolverImpl + Default>() {
    // The release build has the higher build number, so it wins by default.
    let release = PackageBuilder::new("foo")
        .version("1.0.0")
        .build_string("release_1")
        .build_number(1)
        .flags(["release"])
        .build();
    let debug = PackageBuilder::new("foo")
        .version("1.0.0")
        .build_string("debug_0")
        .build_number(0)
        .flags(["debug"])
        .build();
    let debug_cuda = PackageBuilder::new("foo")
        .version("1.0.0")
        .build_string("debug_cuda_0")
        .build_number(0)
        .flags(["debug", "cuda"])
        .build();
    let newer_release = PackageBuilder::new("foo")
        .version("2.0.0")
        .build_string("release_0")
        .flags(["release"])
        .build();
    let consumer = PackageBuilder::new("consumer")
        .version("1.0.0")
        .depends(["foo[flags=[release]]"])
        .build();

    run_solver_cases::<T>(&[
        SolverCase::new("without preferences the highest build number wins")
            .repository(vec![release.clone(), debug.clone()])
            .specs(["foo"])
            .expect_present([&release])
            .expect_absent([&debug]),
        SolverCase::new("a preferred flag outranks the build number")
            .repository(vec![release.clone(), debug.clone()])
            .specs(["foo"])
            .flag_preferences([prefer("debug")])
            .expect_present([&debug])
            .expect_absent([&release]),
        SolverCase::new("an avoided flag ranks below the build number")
            .repository(vec![release.clone(), debug.clone()])
            .specs(["foo"])
            .flag_preferences([avoid("release")])
            .expect_present([&debug])
            .expect_absent([&release]),
        SolverCase::new("earlier preferences take precedence")
            .repository(vec![release.clone(), debug.clone(), debug_cuda.clone()])
            .specs(["foo"])
            .flag_preferences([prefer("debug"), avoid("cuda")])
            .expect_present([&debug])
            .expect_absent([&release, &debug_cuda]),
        SolverCase::new("a later preference still decides among the rest")
            .repository(vec![release.clone(), debug.clone(), debug_cuda.clone()])
            .specs(["foo"])
            .flag_preferences([prefer("debug"), prefer("cuda")])
            .expect_present([&debug_cuda])
            .expect_absent([&release, &debug]),
        SolverCase::new("a higher version wins over a preferred flag")
            .repository(vec![debug.clone(), newer_release.clone()])
            .specs(["foo"])
            .flag_preferences([prefer("debug")])
            .expect_present([&newer_release])
            .expect_absent([&debug]),
        SolverCase::new("a preference falls back when a spec requires another flag")
            .repository(vec![consumer.clone(), release.clone(), debug.clone()])
            .specs(["consumer"])
            .flag_preferences([prefer("debug")])
            .expect_present([&consumer, &release])
            .expect_absent([&debug]),
    ]);
}

#[cfg(feature = "libsolv_c")]
pub(super) fn solve_flag_preferences_are_unsupported<T: SolverImpl + Default>() {
    let pkg = PackageBuilder::new("foo")
        .version("1.0.0")
        .flags(["debug"])
        .build();
    let repository = vec![pkg];
    let task = SolverTask {
        specs: vec![parse_v3_spec("foo")],
        flag_preferences: vec![prefer("debug")],
        ..SolverTask::from_iter([&repository])
    };

    match T::default().solve(task).unwrap_err() {
        SolveError::UnsupportedOperations(operations) => {
            assert_eq!(operations, ["flag_preferences"]);
        }
        other => panic!("expected flag preferences to be unsupported, got {other}"),
    }
}

#[cfg(feature = "libsolv_c")]
fn parse_v3_spec(spec: &str) -> MatchSpec {
    MatchSpec::from_str(
        spec,
        ParseMatchSpecOptions::lenient()
            .with_repodata_revision(RepodataRevision::V3)
            .with_conditionals(true),
    )
    .unwrap()
}

#[cfg(feature = "libsolv_c")]
fn assert_matchspec_flags_unsupported(error: SolveError) {
    match error {
        SolveError::UnsupportedOperations(operations) => {
            assert_eq!(operations, ["matchspec flags"]);
        }
        other => panic!("expected matchspec flags to be unsupported, got {other}"),
    }
}

#[cfg(feature = "libsolv_c")]
pub(super) fn solve_root_flags_are_unsupported<T: SolverImpl + Default>() {
    let pkg = PackageBuilder::new("torch")
        .version("1.0.0")
        .flags(["cuda"])
        .build();
    let repository = vec![pkg];
    let task = SolverTask {
        specs: vec![parse_v3_spec("torch[flags=[cuda]]")],
        ..SolverTask::from_iter([&repository])
    };

    let error = T::default().solve(task).unwrap_err();
    assert_matchspec_flags_unsupported(error);
}

#[cfg(feature = "libsolv_c")]
pub(super) fn solve_dependency_flags_are_unsupported<T: SolverImpl + Default>() {
    let provider = PackageBuilder::new("blas-provider")
        .version("1.0.0")
        .flags(["blas:mkl"])
        .build();
    let consumer = PackageBuilder::new("consumer")
        .version("1.0.0")
        .depends(["blas-provider[flags=[blas:mkl]]"])
        .build();
    let repository = vec![consumer, provider];
    let task = SolverTask {
        specs: vec![parse_v3_spec("consumer")],
        ..SolverTask::from_iter([&repository])
    };

    let error = T::default().solve(task).unwrap_err();
    assert_matchspec_flags_unsupported(error);
}

#[cfg(feature = "libsolv_c")]
pub(super) fn solve_condition_flags_are_unsupported<T: SolverImpl + Default>() {
    let provider = PackageBuilder::new("provider")
        .version("1.0.0")
        .flags(["cuda"])
        .build();
    let helper = PackageBuilder::new("helper").version("1.0.0").build();
    let repository = vec![helper, provider];
    let mut conditional_spec = parse_v3_spec("helper");
    conditional_spec.condition = Some(MatchSpecCondition::MatchSpec(Box::new(parse_v3_spec(
        "provider[flags=[cuda]]",
    ))));
    let task = SolverTask {
        specs: vec![conditional_spec],
        ..SolverTask::from_iter([&repository])
    };

    let error = T::default().solve(task).unwrap_err();
    assert_matchspec_flags_unsupported(error);
}
