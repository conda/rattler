use std::time::Duration;

use jiff::Timestamp;
use rattler_conda_types::{RepoDataRecord, package::CondaArchiveType};
use rattler_solve::{ExcludeNewer, SolverImpl, SolverTask, TimestampPolicy};

use crate::helpers::PackageBuilder;

const OLD: &str = "2020-01-01T00:00:00Z";
const CUTOFF: &str = "2026-03-23T00:00:00Z";
const NEW: &str = "2026-03-24T00:00:00Z";

fn record(build: Option<&str>, indexed: Option<&str>) -> RepoDataRecord {
    let mut record = PackageBuilder::new("foo").channel("test").build();
    record.package_record.timestamp = build.map(|ts| ts.parse::<Timestamp>().unwrap().into());
    record.package_record.indexed_timestamp =
        indexed.map(|ts| ts.parse::<Timestamp>().unwrap().into());
    record
}

fn assert_allowed<T: SolverImpl + Default>(
    record: RepoDataRecord,
    config: ExcludeNewer,
    allowed: bool,
) {
    let records = vec![record];
    let result = T::default().solve(SolverTask {
        specs: vec!["foo".parse().unwrap()],
        exclude_newer: Some(config.clone()),
        ..SolverTask::from_iter([&records])
    });
    assert_eq!(
        result.is_ok(),
        allowed,
        "{config:?}: {records:?}: {result:?}"
    );
}

pub fn missing_timestamps<T: SolverImpl + Default>() {
    use TimestampPolicy::*;
    // Explicit expectations: no timestamp, build time only, index time only.
    for (policy, missing_allowed, build_allowed) in [
        (AllowMissing, true, true),
        (RequireTimestamp, false, true),
        (RequireIndexedTimestamp, false, false),
    ] {
        let config =
            ExcludeNewer::from_datetime(CUTOFF.parse().unwrap()).with_timestamp_policy(policy);
        assert_allowed::<T>(record(None, None), config.clone(), missing_allowed);
        assert_allowed::<T>(record(Some(OLD), None), config.clone(), build_allowed);
        assert_allowed::<T>(record(None, Some(OLD)), config, true);
    }
}

pub fn exemption_cutoffs<T: SolverImpl + Default>() {
    use TimestampPolicy::*;
    for (policy, missing_allowed, build_allowed) in [
        (AllowMissing, true, true),
        (RequireTimestamp, false, true),
        (RequireIndexedTimestamp, false, false),
    ] {
        let config = ExcludeNewer::from_datetime(OLD.parse().unwrap())
            .with_timestamp_policy(policy)
            .with_exemption_until("foo".parse().unwrap(), CUTOFF.parse().unwrap())
            .unwrap();
        // Records up to the cutoff of the exemption are exempt.
        assert_allowed::<T>(record(None, Some(CUTOFF)), config.clone(), true);
        assert_allowed::<T>(record(Some(CUTOFF), None), config.clone(), build_allowed);
        // Records after it fall back to the cutoff of the package.
        assert_allowed::<T>(record(None, Some(NEW)), config.clone(), false);
        assert_allowed::<T>(record(Some(OLD), Some(NEW)), config.clone(), false);
        // A record without a timestamp cannot be shown to be older than the
        // cutoff of the exemption, only the policy can allow it.
        assert_allowed::<T>(record(None, None), config, missing_allowed);
    }
}

pub fn indexed_timestamp_cutoffs<T: SolverImpl + Default>() {
    for policy in [
        TimestampPolicy::AllowMissing,
        TimestampPolicy::RequireTimestamp,
        TimestampPolicy::RequireIndexedTimestamp,
    ] {
        let config =
            ExcludeNewer::from_datetime(CUTOFF.parse().unwrap()).with_timestamp_policy(policy);
        // Publication time takes precedence in both directions.
        assert_allowed::<T>(record(Some(OLD), Some(NEW)), config.clone(), false);
        assert_allowed::<T>(record(Some(NEW), Some(OLD)), config.clone(), true);
        assert_allowed::<T>(record(None, Some(CUTOFF)), config.clone(), true);
        assert_allowed::<T>(
            record(None, Some("2026-03-23T00:00:00.001Z")),
            config,
            false,
        );

        let duration =
            ExcludeNewer::from_duration_with_now(Duration::from_secs(86400), NEW.parse().unwrap())
                .with_timestamp_policy(policy);
        assert_allowed::<T>(record(None, Some(CUTOFF)), duration.clone(), true);
        assert_allowed::<T>(record(Some(OLD), Some(NEW)), duration, false);
    }
}

#[allow(deprecated)] // Tests the deprecated package cutoff overrides.
pub fn timestamp_overrides<T: SolverImpl + Default>() {
    let config = ExcludeNewer::from_datetime(CUTOFF.parse().unwrap())
        .with_timestamp_policy(TimestampPolicy::RequireIndexedTimestamp)
        .with_channel_cutoff("test", NEW.parse().unwrap());
    assert_allowed::<T>(record(Some(OLD), Some(NEW)), config.clone(), true);
    // A package cutoff takes precedence over the channel cutoff.
    let config = config.with_package_cutoff("foo".parse().unwrap(), CUTOFF.parse().unwrap());
    assert_allowed::<T>(record(Some(OLD), Some(NEW)), config, false);

    let config = ExcludeNewer::from_datetime(CUTOFF.parse().unwrap())
        .with_timestamp_policy(TimestampPolicy::RequireIndexedTimestamp)
        .with_channel_duration_with_now("test", Duration::ZERO, NEW.parse().unwrap());
    assert_allowed::<T>(record(None, Some(NEW)), config.clone(), true);
    let config = config.with_package_duration_with_now(
        "foo".parse().unwrap(),
        Duration::from_secs(86400),
        NEW.parse().unwrap(),
    );
    assert_allowed::<T>(record(None, Some(NEW)), config.clone(), false);
    // Cutoff overrides never relax the global metadata requirement.
    assert_allowed::<T>(record(Some(OLD), None), config, false);
}

pub fn timestamp_archive_fallback<T: SolverImpl + Default>() {
    use TimestampPolicy::*;
    for (policy, indexed, expect_tar) in [
        (AllowMissing, Some(NEW), true),
        (RequireTimestamp, Some(NEW), true),
        (RequireIndexedTimestamp, Some(NEW), true),
        (AllowMissing, None, false),
        (RequireTimestamp, None, false),
        (RequireIndexedTimestamp, None, true),
    ] {
        let conda = record(Some(OLD), indexed);
        let mut tar = PackageBuilder::new("foo")
            .archive_type(CondaArchiveType::TarBz2)
            .build();
        tar.package_record.indexed_timestamp = Some(OLD.parse::<Timestamp>().unwrap().into());
        // Deduplication must work regardless of input order.
        for records in [
            vec![conda.clone(), tar.clone()],
            vec![tar.clone(), conda.clone()],
        ] {
            let result = T::default()
                .solve(SolverTask {
                    specs: vec!["foo".parse().unwrap()],
                    exclude_newer: Some(
                        ExcludeNewer::from_datetime(CUTOFF.parse().unwrap())
                            .with_timestamp_policy(policy),
                    ),
                    ..SolverTask::from_iter([&records])
                })
                .unwrap();
            let expected = if expect_tar { &tar } else { &conda };
            assert_eq!(
                result.records[0].identifier, expected.identifier,
                "{policy:?}, indexed={indexed:?}"
            );
        }
    }
}

/// A locked package only expresses a preference, so it must not bring a
/// record newer than the cutoff back into the solve. The solver falls back to
/// the newest record that the cutoff allows instead.
pub fn locked_package_newer_than_cutoff<T: SolverImpl + Default>() {
    let mut old = PackageBuilder::new("foo").version("1").build();
    old.package_record.indexed_timestamp = Some(OLD.parse::<Timestamp>().unwrap().into());
    let mut new = PackageBuilder::new("foo").version("2").build();
    new.package_record.indexed_timestamp = Some(NEW.parse::<Timestamp>().unwrap().into());
    let records = vec![old, new.clone()];

    let solve = |exclude_newer: Option<ExcludeNewer>| {
        T::default()
            .solve(SolverTask {
                specs: vec!["foo".parse().unwrap()],
                locked_packages: vec![&new],
                exclude_newer,
                ..SolverTask::from_iter([&records])
            })
            .unwrap()
            .records[0]
            .package_record
            .version
            .to_string()
    };

    assert_eq!(solve(None), "2");
    assert_eq!(
        solve(Some(ExcludeNewer::from_datetime(CUTOFF.parse().unwrap()))),
        "1"
    );
}

#[test]
fn default_timestamp_policy() {
    let config = ExcludeNewer::from_datetime(CUTOFF.parse().unwrap());
    assert_eq!(config.timestamp_policy(), TimestampPolicy::RequireTimestamp);
}

#[cfg(feature = "resolvo")]
#[test]
fn resolvo_timestamp_diagnostics() {
    let cutoff: Timestamp = "2026-03-23T00:00:00Z".parse().unwrap();
    for (policy, build, indexed, message) in [
        (
            TimestampPolicy::RequireIndexedTimestamp,
            Some(cutoff),
            None,
            "the package has no indexed timestamp",
        ),
        (
            TimestampPolicy::RequireTimestamp,
            None,
            None,
            "the package has no timestamp",
        ),
        (
            TimestampPolicy::AllowMissing,
            None,
            Some(
                cutoff
                    .checked_add(jiff::Span::new().milliseconds(1))
                    .unwrap(),
            ),
            "the package is uploaded after the cutoff date",
        ),
    ] {
        let mut record = PackageBuilder::new("foo").build();
        record.package_record.timestamp = build.map(Into::into);
        record.package_record.indexed_timestamp = indexed.map(Into::into);
        let records = vec![record];
        let error = rattler_solve::resolvo::Solver
            .solve(SolverTask {
                specs: vec!["foo".parse().unwrap()],
                exclude_newer: Some(
                    ExcludeNewer::from_datetime(cutoff).with_timestamp_policy(policy),
                ),
                ..SolverTask::from_iter([&records])
            })
            .unwrap_err();
        assert!(error.to_string().contains(message), "{error}");
    }
}

/// A pinned package that is newer than the cutoff makes the solve
/// unsatisfiable, the same way as a pinned package that is excluded through
/// `excluded_candidates`. Dropping the pin would let the solver quietly pick
/// a different version of a package that was pinned on purpose.
#[cfg(feature = "resolvo")]
#[test]
fn resolvo_pinned_package_newer_than_cutoff() {
    let mut old = PackageBuilder::new("foo").version("1").build();
    old.package_record.indexed_timestamp = Some(OLD.parse::<Timestamp>().unwrap().into());
    let mut new = PackageBuilder::new("foo").version("2").build();
    new.package_record.indexed_timestamp = Some(NEW.parse::<Timestamp>().unwrap().into());
    let records = vec![old, new.clone()];

    let error = rattler_solve::resolvo::Solver
        .solve(SolverTask {
            specs: vec!["foo".parse().unwrap()],
            pinned_packages: vec![&new],
            exclude_newer: Some(ExcludeNewer::from_datetime(CUTOFF.parse().unwrap())),
            ..SolverTask::from_iter([&records])
        })
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("the package is uploaded after the cutoff date"),
        "{error}"
    );
}
