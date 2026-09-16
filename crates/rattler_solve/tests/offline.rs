//! The offline path from one end to the other: index the package cache,
//! derive exclusions from it, solve, and install the result from the cache
//! without a single fetch.
//!
//! Each half of the chain has focused tests next to its implementation. What
//! only this test pins is their agreement: an exclusion map derived from a
//! [`CacheIndex`] must steer the solver to records the cache can serve.

#![cfg(feature = "resolvo")]

use std::{collections::HashMap, path::PathBuf, str::FromStr, sync::Arc};

use rattler_cache::package_cache::{CacheIndex, PackageCache};
use rattler_conda_types::{
    GenericVirtualPackage, MatchSpec, PackageName, PackageRecord, ParseStrictness, RepoDataRecord,
    Version, VersionWithSource,
    package::{ArchiveIdentifier, CondaArchiveIdentifier, CondaArchiveType, DistArchiveType},
};
use rattler_digest::{Sha256, compute_file_digest};
use rattler_solve::{SolverImpl, SolverTask, resolvo::Solver};
use url::Url;

fn test_data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
}

/// A remote record for a package of the given version, sharing the name,
/// build string and hash of the cached test archive unless the version says
/// otherwise.
fn remote_record(version: &str, sha256: Option<rattler_digest::Sha256Hash>) -> RepoDataRecord {
    let mut package_record = PackageRecord::new(
        PackageName::new_unchecked("clobber-python"),
        VersionWithSource::from(Version::from_str(version).unwrap()),
        "cpython".to_string(),
    );
    package_record.sha256 = sha256;

    RepoDataRecord {
        url: Url::parse(&format!(
            "https://example.com/linux-64/clobber-python-{version}-cpython.conda"
        ))
        .unwrap(),
        channel: None,
        identifier: rattler_conda_types::package::DistArchiveIdentifier {
            identifier: ArchiveIdentifier {
                name: "clobber-python".to_string(),
                version: version.to_string(),
                build_string: "cpython".to_string(),
            },
            archive_type: DistArchiveType::Conda(CondaArchiveType::Conda),
        },
        package_record,
    }
}

/// Builds the exclusion map the way an offline caller does: everything the
/// cache cannot serve is ruled out, with a reason.
fn offline_exclusions<'a>(
    index: &CacheIndex,
    records: impl IntoIterator<Item = &'a RepoDataRecord>,
) -> HashMap<Url, Arc<str>> {
    let reason: Arc<str> = Arc::from("not available locally");
    records
        .into_iter()
        .filter(|record| !index.contains_record(record))
        .map(|record| (record.url.clone(), Arc::clone(&reason)))
        .collect()
}

#[tokio::test]
async fn test_offline_solve_installs_from_the_cache_alone() {
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = PackageCache::new(cache_dir.path());
    let package_path = test_data_dir().join("clobber/clobber-python-0.1.0-cpython.conda");
    let sha256 = compute_file_digest::<Sha256>(&package_path).unwrap();

    // Only version 0.1.0 is in the cache.
    let mut cached_record = PackageRecord::new(
        PackageName::new_unchecked("clobber-python"),
        "0.1.0".parse::<VersionWithSource>().unwrap(),
        "cpython".to_string(),
    );
    cached_record.sha256 = Some(sha256);
    cache
        .get_or_fetch_from_path(&package_path, Some(&cached_record), None)
        .await
        .unwrap();

    // The repodata offers a newer version as well, which the solver would
    // normally prefer.
    let records = vec![
        remote_record("0.1.0", Some(sha256)),
        remote_record("0.2.0", None),
    ];

    let index = cache.index().await.unwrap();
    let excluded_candidates = offline_exclusions(&index, &records);
    assert_eq!(
        excluded_candidates.len(),
        1,
        "only the uncached newer version is ruled out"
    );

    let task = SolverTask {
        specs: vec![MatchSpec::from_str("clobber-python", ParseStrictness::Lenient).unwrap()],
        virtual_packages: Vec::<GenericVirtualPackage>::new(),
        excluded_candidates,
        ..SolverTask::from_iter([&records])
    };
    let result = Solver.solve(task).unwrap();

    let chosen = result
        .records
        .iter()
        .find(|record| record.package_record.name.as_normalized() == "clobber-python")
        .expect("the solve contains the requested package");
    assert_eq!(
        chosen.package_record.version.to_string(),
        "0.1.0",
        "the solver has to settle for the cached version"
    );

    // Everything the solve picked installs from the cache alone: the fetch
    // callback panics if the cache cannot serve the package.
    for record in &result.records {
        let key = rattler_cache::package_cache::CacheKey::from(
            CondaArchiveIdentifier::try_from_url(&record.url).unwrap(),
        )
        .with_opt_sha256(record.package_record.sha256);

        cache
            .get_or_fetch(
                key,
                |_destination| async move {
                    Err::<(), std::io::Error>(std::io::Error::other(
                        "the offline install must never fetch",
                    ))
                },
                None,
            )
            .await
            .expect("the cached package is served without fetching");
    }
}
