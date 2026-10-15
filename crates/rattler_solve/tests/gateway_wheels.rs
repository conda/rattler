//! Exercise wheel records through the real gateway before passing them to Resolvo.
#![cfg(feature = "resolvo")]

use rattler_conda_types::{
    Channel, PackageName, Subdir,
    package::{DistArchiveType, WheelArchiveType},
};
use rattler_repodata_gateway::Gateway;
use rattler_solve::{SolveError, SolverImpl, SolverTask};
use rstest::rstest;

/// The wheel is the only root requirement. Its dependency must be loaded by the
/// gateway and enforced by the solver, not supplied as an additional root spec.
#[rstest]
#[case::satisfiable(true)]
#[case::unsatisfiable(false)]
#[tokio::test]
async fn solve_wheel_from_gateway(#[case] include_compatible_helper: bool) {
    let root = tempfile::tempdir().unwrap();
    let subdir = root.path().join("noarch");
    std::fs::create_dir_all(&subdir).unwrap();
    let wheel_url = "https://example.org/wheel_only-1.0-py3-none-any.whl";
    let mut repodata = serde_json::json!({
        "info": { "subdir": "noarch" },
        "repodata_version": 3,
        "packages.conda": {
            "helper-1.0-0.conda": {
                "name": "helper", "version": "1.0", "build": "0",
                "build_number": 0, "subdir": "noarch", "depends": []
            }
        },
        "v3": { "whl": {
            "wheel-only-1.0-py3_none_any_0": {
                "name": "wheel-only", "version": "1.0", "build": "py3_none_any_0",
                "build_number": 0, "subdir": "noarch", "depends": ["helper >=2"],
                "fn": "wheel_only-1.0-py3-none-any.whl", "url": wheel_url
            }
        }}
    });
    if include_compatible_helper {
        repodata["packages.conda"]["helper-2.0-0.conda"] = serde_json::json!({
            "name": "helper", "version": "2.0", "build": "0",
            "build_number": 0, "subdir": "noarch", "depends": []
        });
    }
    std::fs::write(subdir.join("repodata.json"), repodata.to_string()).unwrap();

    let cache = tempfile::tempdir().unwrap();
    let gateway = Gateway::builder().with_cache_dir(cache.path()).finish();
    let available_packages = gateway
        .query(
            [Channel::try_from_directory(root.path()).unwrap()],
            [Subdir::NoArch],
            ["wheel-only".parse::<PackageName>().unwrap()],
        )
        .recursive(true)
        .await
        .unwrap();

    // Assert that an unsatisfiable result cannot be explained by the wheel
    // itself being filtered out before it reaches the solver.
    let candidates: Vec<_> = available_packages
        .iter()
        .flat_map(|repo| repo.iter())
        .collect();
    assert!(
        candidates
            .iter()
            .any(|r| r.package_record.name.as_normalized() == "wheel-only")
    );
    assert!(candidates.iter().any(|r| {
        r.package_record.name.as_normalized() == "helper"
            && r.package_record.version.to_string() == "1.0"
    }));
    assert_eq!(
        candidates.len(),
        if include_compatible_helper { 3 } else { 2 }
    );

    let result = rattler_solve::resolvo::Solver.solve(SolverTask {
        specs: vec!["wheel-only".parse().unwrap()],
        ..SolverTask::from_iter(&available_packages)
    });

    if !include_compatible_helper {
        assert!(matches!(result, Err(SolveError::Unsolvable(_))));
        return;
    }

    let solution = result.unwrap().records;
    assert_eq!(solution.len(), 2);
    let wheel = solution
        .iter()
        .find(|r| r.package_record.name.as_normalized() == "wheel-only")
        .unwrap();
    assert_eq!(wheel.package_record.version.to_string(), "1.0");
    assert_eq!(
        wheel.identifier.archive_type,
        DistArchiveType::Wheel(WheelArchiveType::Whl)
    );
    assert_eq!(
        wheel.identifier.to_file_name(),
        "wheel-only-1.0-py3_none_any_0.whl"
    );
    assert_eq!(wheel.url.as_str(), wheel_url);
    assert_eq!(wheel.package_record.depends, ["helper >=2"]);

    let helper = solution
        .iter()
        .find(|r| r.package_record.name.as_normalized() == "helper")
        .unwrap();
    assert_eq!(helper.package_record.version.to_string(), "2.0");
    assert_eq!(helper.identifier.to_file_name(), "helper-2.0-0.conda");
}
