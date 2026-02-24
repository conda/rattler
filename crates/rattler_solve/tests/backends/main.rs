use std::{collections::BTreeMap, str::FromStr, time::Instant};

use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use rattler_conda_types::{
    package::{ArchiveIdentifier, CondaArchiveType, DistArchiveIdentifier, DistArchiveType},
    Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, NoArchType, PackageName,
    PackageRecord, ParseMatchSpecOptions, ParseStrictness, RepoData, RepoDataRecord, SolverResult,
    Version,
};
use rattler_repodata_gateway::sparse::{PackageFormatSelection, SparseRepoData};
use rattler_solve::{
    ChannelPriority, MinimumAgeConfig, SolveError, SolveStrategy, SolverImpl, SolverTask,
};
use url::Url;

mod conditional_tests;
mod extras_tests;
mod helpers;
mod min_age_tests;
mod solver_case_tests;
mod strategy_tests;

fn channel_config() -> ChannelConfig {
    ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap())
}

fn pytorch_json_path() -> String {
    format!(
        "{}/{}",
        env!("CARGO_MANIFEST_DIR"),
        "../../test-data/channels/pytorch/linux-64/repodata.json"
    )
}

fn dummy_channel_json_path() -> String {
    format!(
        "{}/{}",
        env!("CARGO_MANIFEST_DIR"),
        "../../test-data/channels/dummy/linux-64/repodata.json"
    )
}

fn dummy_channel_with_optional_dependencies_json_path() -> String {
    format!(
        "{}/{}",
        env!("CARGO_MANIFEST_DIR"),
        "../../test-data/channels/dummy-optional-dependencies/noarch/repodata.json"
    )
}

pub(crate) fn dummy_md5_hash() -> rattler_digest::Md5Hash {
    rattler_digest::parse_digest_from_hex::<rattler_digest::Md5>("b3af409bb8423187c75e6c7f5b683908")
        .unwrap()
}

pub(crate) fn dummy_sha256_hash() -> rattler_digest::Sha256Hash {
    rattler_digest::parse_digest_from_hex::<rattler_digest::Sha256>(
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    )
    .unwrap()
}

fn read_repodata(path: &str) -> Vec<RepoDataRecord> {
    let repo_data: RepoData =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    repo_data.into_repo_data_records(&Channel::from_str("conda-forge", &channel_config()).unwrap())
}

fn read_sparse_repodata(path: &str) -> SparseRepoData {
    SparseRepoData::from_file(
        Channel::from_str("dummy", &channel_config()).unwrap(),
        "dummy".to_string(),
        path,
        None,
    )
    .unwrap()
}

fn installed_package(
    channel: &str,
    subdir: &str,
    name: &str,
    version: &str,
    build: &str,
    build_number: u64,
) -> RepoDataRecord {
    PackageBuilder::new(name)
        .channel(channel)
        .subdir(subdir)
        .version(version)
        .build_string(build)
        .build_number(build_number)
        .build()
}

#[derive(Clone)]
struct PackageBuilder {
    record: RepoDataRecord,
}

impl PackageBuilder {
    fn new(name: &str) -> Self {
        Self {
            record: RepoDataRecord {
                url: Url::from_str("http://example.com").unwrap(),
                channel: None,
                identifier: DistArchiveIdentifier {
                    identifier: ArchiveIdentifier {
                        name: name.to_string(),
                        version: "0.0.0".to_string(),
                        build_string: "h123456_0".to_string(),
                    },
                    archive_type: DistArchiveType::Conda(CondaArchiveType::Conda),
                },
                package_record: PackageRecord {
                    name: name.parse().unwrap(),
                    version: Version::from_str("0.0.0").unwrap().into(),
                    build: "h123456_0".to_string(),
                    build_number: 0,
                    subdir: "linux-64".to_string(),
                    md5: Some(dummy_md5_hash()),
                    sha256: Some(dummy_sha256_hash()),
                    size: None,
                    arch: None,
                    experimental_extra_depends: BTreeMap::new(),
                    platform: None,
                    depends: Vec::new(),
                    constrains: Vec::new(),
                    track_features: Vec::new(),
                    features: None,
                    noarch: NoArchType::default(),
                    license: None,
                    license_family: None,
                    timestamp: None,
                    legacy_bz2_size: None,
                    legacy_bz2_md5: None,
                    purls: None,
                    python_site_packages_path: None,
                    run_exports: None,
                },
            },
        }
    }

    #[allow(dead_code)]
    fn depends(mut self, deps: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.record.package_record.depends = deps.into_iter().map(Into::into).collect();
        self
    }

    fn channel(mut self, channel: &str) -> Self {
        self.record.channel = Some(channel.to_string());
        self
    }

    fn subdir(mut self, subdir: &str) -> Self {
        self.record.package_record.subdir = subdir.to_string();
        self
    }

    fn version(mut self, version: &str) -> Self {
        self.record.package_record.version = Version::from_str(version).unwrap().into();
        self
    }

    fn build_string(mut self, build: &str) -> Self {
        self.record.package_record.build = build.to_string();
        self
    }

    fn build_number(mut self, build_number: u64) -> Self {
        self.record.package_record.build_number = build_number;
        self
    }

    fn build(self) -> RepoDataRecord {
        self.record
    }
}

impl From<PackageBuilder> for RepoDataRecord {
    fn from(builder: PackageBuilder) -> Self {
        builder.build()
    }
}

fn solve_real_world<T: SolverImpl + Default>(specs: Vec<&str>) -> Vec<String> {
    let specs = specs
        .iter()
        .map(|s| MatchSpec::from_str(s, ParseStrictness::Lenient).unwrap())
        .collect::<Vec<_>>();

    let sparse_repo_data = read_real_world_repo_data();

    let names = specs.iter().filter_map(|s| {
        s.name
            .as_ref()
            .and_then(|n| Option::<PackageName>::from(n.clone()))
    });
    let available_packages = SparseRepoData::load_records_recursive(
        sparse_repo_data,
        names,
        None,
        PackageFormatSelection::default(),
    )
    .unwrap();

    let solver_task = SolverTask {
        specs: specs.clone(),
        ..SolverTask::from_iter(&available_packages)
    };

    let pkgs1 = match T::default().solve(solver_task) {
        Ok(result) => result.records,
        Err(e) => panic!("{e}"),
    };

    let extract_pkgs = |records: Vec<RepoDataRecord>| {
        let mut pkgs = records
            .into_iter()
            .map(|pkg| {
                format!(
                    "{} {} {}",
                    pkg.package_record.name.as_normalized(),
                    pkg.package_record.version,
                    pkg.package_record.build
                )
            })
            .collect::<Vec<_>>();

        // The order of packages is nondeterministic, so we sort them to ensure we can
        // compare them to a previous run
        pkgs.sort();
        pkgs
    };

    extract_pkgs(pkgs1)
}

fn read_real_world_repo_data() -> &'static Vec<SparseRepoData> {
    static REPO_DATA: Lazy<Vec<SparseRepoData>> = Lazy::new(|| {
        let json_file = tools::fetch_test_conda_forge_repodata("linux-64")
            .expect("Failed to fetch linux-64 repodata");
        let json_file_noarch = tools::fetch_test_conda_forge_repodata("noarch")
            .expect("Failed to fetch noarch repodata");

        vec![
            read_sparse_repodata(json_file.to_str().unwrap()),
            read_sparse_repodata(json_file_noarch.to_str().unwrap()),
        ]
    });

    &REPO_DATA
}

fn read_pytorch_sparse_repo_data() -> &'static SparseRepoData {
    static REPO_DATA: Lazy<SparseRepoData> = Lazy::new(|| {
        let pytorch = pytorch_json_path();
        SparseRepoData::from_file(
            Channel::from_str("pytorch", &channel_config()).unwrap(),
            "pytorch".to_string(),
            pytorch,
            None,
        )
        .unwrap()
    });

    &REPO_DATA
}

fn read_conda_forge_sparse_repo_data() -> &'static SparseRepoData {
    static REPO_DATA: Lazy<SparseRepoData> = Lazy::new(|| {
        let conda_forge = tools::fetch_test_conda_forge_repodata("linux-64")
            .expect("Failed to fetch linux-64 repodata");
        SparseRepoData::from_file(
            Channel::from_str("conda-forge", &channel_config()).unwrap(),
            "conda-forge".to_string(),
            &conda_forge,
            None,
        )
        .unwrap()
    });
    &REPO_DATA
}
macro_rules! solver_backend_tests {
    ($T:path) => {
        use chrono::{DateTime, Utc};
        use itertools::Itertools;

        #[test]
        fn test_solve_quetz() {
            insta::assert_yaml_snapshot!(solve_real_world::<$T>(vec!["quetz",]));
        }

        #[test]
        fn test_solve_xtensor_xsimd() {
            insta::assert_yaml_snapshot!(solve_real_world::<$T>(vec!["xtensor", "xsimd",]));
        }

        #[test]
        fn test_solve_tensorflow() {
            insta::assert_yaml_snapshot!(solve_real_world::<$T>(vec!["tensorflow"]));
        }

        #[test]
        fn test_solve_tensorboard() {
            insta::assert_yaml_snapshot!(solve_real_world::<$T>(vec![
                "tensorboard=2.1.1",
                "grpc-cpp=1.39.1"
            ]));
        }

        #[test]
        fn test_solve_python() {
            insta::assert_yaml_snapshot!(solve_real_world::<$T>(vec!["python=3.9"]));
        }

        #[test]
        fn test_solve_python_numpy() {
            insta::assert_yaml_snapshot!(solve_real_world::<$T>(vec![
                "numpy==1.23.2",
                "scipy==1.8.1",
                "python=3.9.*"
            ]));
        }

        #[test]
        fn test_solve_with_error() {
            let result = solve::<$T>(
                &[dummy_channel_json_path()],
                SimpleSolveTask {
                    specs: &["foobar >=2", "bors >= 2"],
                    ..SimpleSolveTask::default()
                },
            );

            assert!(result.is_err());

            let err = result.err().unwrap();
            insta::assert_snapshot!(err);
        }

        #[test]
        fn test_solve_dummy_repo_install_non_existent() {
            let result = solve::<$T>(
                &[dummy_channel_json_path()],
                SimpleSolveTask {
                    specs: &["asdfasdf", "foo<4"],
                    ..SimpleSolveTask::default()
                },
            );

            assert!(result.is_err());

            let err = result.err().unwrap();
            insta::assert_debug_snapshot!(err);
        }

        #[test]
        fn test_solve_dummy_repo_missing_virtual_package() {
            let result = solve::<$T>(
                &[dummy_channel_json_path()],
                SimpleSolveTask {
                    specs: &["bar"],
                    ..SimpleSolveTask::default()
                },
            );

            assert!(matches!(result.err(), Some(SolveError::Unsolvable(_))));
        }

        #[test]
        fn test_solve_dummy_repo_with_virtual_package() {
            let pkgs = solve::<$T>(
                &[dummy_channel_json_path()],
                SimpleSolveTask {
                    specs: &["bar"],
                    virtual_packages: vec![GenericVirtualPackage {
                        name: rattler_conda_types::PackageName::new_unchecked("__unix"),
                        version: Version::from_str("0").unwrap(),
                        build_string: "0".to_string(),
                    }],
                    ..SimpleSolveTask::default()
                },
            )
            .unwrap();

            assert_eq!(pkgs.records.len(), 1);

            let info = &pkgs.records[0];
            assert_eq!("bar", info.package_record.name.as_normalized());
            assert_eq!("1.2.3", &info.package_record.version.to_string());
        }

        #[test]
        fn test_solve_dummy_repo_install_new() {
            let pkgs = solve::<$T>(
                &[dummy_channel_json_path()],
                SimpleSolveTask {
                    specs: &["foo<4"],
                    ..SimpleSolveTask::default()
                },
            )
            .unwrap();

            assert_eq!(1, pkgs.records.len());
            let info = &pkgs.records[0];

            assert_eq!(
                "foo-3.0.2-py36h1af98f8_3.conda",
                info.identifier.to_string()
            );
            assert_eq!(
                "https://conda.anaconda.org/conda-forge/linux-64/foo-3.0.2-py36h1af98f8_3.conda",
                info.url.to_string()
            );
            assert_eq!(
                Some("https://conda.anaconda.org/conda-forge/"),
                info.channel.as_deref()
            );
            assert_eq!("foo", info.package_record.name.as_normalized());
            assert_eq!("linux-64", info.package_record.subdir);
            assert_eq!("3.0.2", info.package_record.version.to_string());
            assert_eq!("py36h1af98f8_3", info.package_record.build);
            assert_eq!(3, info.package_record.build_number);
            assert_eq!(
                rattler_digest::parse_digest_from_hex::<rattler_digest::Sha256>(
                    "67a63bec3fd3205170eaad532d487595b8aaceb9814d13c6858d7bac3ef24cd4"
                )
                .as_ref()
                .unwrap(),
                info.package_record.sha256.as_ref().unwrap()
            );
            assert_eq!(
                rattler_digest::parse_digest_from_hex::<rattler_digest::Md5>(
                    "fb731d9290f0bcbf3a054665f33ec94f"
                )
                .as_ref()
                .unwrap(),
                info.package_record.md5.as_ref().unwrap()
            );
        }

        #[test]
        fn test_solve_dummy_repo_prefers_conda_package() {
            // There following package is provided as .tar.bz and as .conda in repodata.json
            let match_spec = "foo=3.0.2=py36h1af98f8_1";

            let operations = solve::<$T>(
                &[dummy_channel_json_path()],
                SimpleSolveTask {
                    specs: &[match_spec],
                    ..SimpleSolveTask::default()
                },
            )
            .unwrap();

            // The .conda entry is selected for installing
            assert_eq!(operations.records.len(), 1);
            assert_eq!(
                operations.records[0].identifier.to_string(),
                "foo-3.0.2-py36h1af98f8_1.conda"
            );
        }

        #[test]
        fn test_min_age_filters_new_packages() {
            crate::min_age_tests::solve_min_age_filters_new_packages::<$T>();
        }

        #[test]
        fn test_min_age_with_exemption() {
            crate::min_age_tests::solve_min_age_with_exemption::<$T>();
        }

        #[test]
        fn test_min_age_with_dependencies() {
            crate::min_age_tests::solve_min_age_with_dependencies::<$T>();
        }

        #[test]
        fn test_min_age_exempt_dependency() {
            crate::min_age_tests::solve_min_age_exempt_dependency::<$T>();
        }

        #[test]
        fn test_min_age_excludes_unknown_timestamp() {
            crate::min_age_tests::solve_min_age_excludes_unknown_timestamp::<$T>();
        }

        #[test]
        fn test_min_age_include_unknown_timestamp() {
            crate::min_age_tests::solve_min_age_include_unknown_timestamp::<$T>();
        }

        #[test]
        fn test_min_age_exempt_no_timestamp() {
            crate::min_age_tests::solve_min_age_exempt_no_timestamp::<$T>();
        }

        #[test]
        fn resolvo_issue_188() {
            crate::solver_case_tests::resolvo_issue_188::<$T>();
        }

        #[test]
        fn test_duplicate_record() {
            use rattler_solve::SolverImpl;

            let mut records = super::read_repodata(&dummy_channel_json_path());
            records.push(records[0].clone());

            let task = rattler_solve::SolverTask::from_iter([&records]);

            let result = <$T>::default().solve(task);
            match result {
                Err(rattler_solve::SolveError::DuplicateRecords(_)) => {}
                _ => panic!("expected a DuplicateRecord error"),
            }
        }

        #[test]
        fn test_virtual_package_constrains() {
            // This tests that a package that has a constrains on a virtual package is
            // properly restricted.
            let result = solve::<$T>(
                &[dummy_channel_json_path()],
                SimpleSolveTask {
                    specs: &["cuda-version"],
                    virtual_packages: vec![GenericVirtualPackage {
                        name: "__cuda".parse().unwrap(),
                        version: Version::from_str("1").unwrap(),
                        build_string: "0".to_string(),
                    }],
                    ..SimpleSolveTask::default()
                },
            );

            let output = match result {
                Ok(pkgs) => pkgs
                    .records
                    .iter()
                    .format_with("\n", |pkg, f| {
                        f(&format_args!(
                            "{}={}={}",
                            pkg.package_record.name.as_normalized(),
                            pkg.package_record.version.as_str(),
                            &pkg.package_record.build
                        ))
                    })
                    .to_string(),
                Err(e) => e.to_string(),
            };

            insta::assert_snapshot!(output);
        }

        #[test]
        fn test_solve_conditional_dependencies() {
            crate::conditional_tests::solve_conditional_dependencies::<$T>();
        }

        #[test]
        fn test_solve_complex_conditional_dependencies() {
            crate::conditional_tests::solve_complex_conditional_dependencies::<$T>();
        }

        #[test]
        fn test_extras_basic() {
            crate::extras_tests::solve_extras_basic::<$T>();
        }

        #[test]
        fn test_extras_version_restriction() {
            crate::extras_tests::solve_extras_version_restriction::<$T>();
        }

        #[test]
        fn test_multiple_extras() {
            crate::extras_tests::solve_multiple_extras::<$T>();
        }

        #[test]
        fn test_extras_complex_constraints() {
            crate::extras_tests::solve_extras_complex_constraints::<$T>();
        }

        #[test]
        fn test_solver_case_favored() {
            crate::solver_case_tests::solve_favored::<$T>();
        }

        #[test]
        fn test_solver_case_constraints() {
            crate::solver_case_tests::solve_constraints::<$T>();
        }

        #[test]
        fn test_solver_case_exclude_newer() {
            crate::solver_case_tests::solve_exclude_newer::<$T>();
        }

        #[test]
        fn test_solver_case_upgrade() {
            crate::solver_case_tests::solve_upgrade::<$T>();
        }

        #[test]
        fn test_solver_case_downgrade() {
            crate::solver_case_tests::solve_downgrade::<$T>();
        }

        #[test]
        fn test_solver_case_install_new() {
            crate::solver_case_tests::solve_install_new::<$T>();
        }

        #[test]
        fn test_solver_case_remove() {
            crate::solver_case_tests::solve_remove::<$T>();
        }

        #[test]
        fn test_solver_case_noop() {
            crate::solver_case_tests::solve_noop::<$T>();
        }

        #[test]
        fn test_conditional_root_requirement_satisfied() {
            crate::conditional_tests::solve_conditional_root_requirement_satisfied::<$T>();
        }

        #[test]
        fn test_conditional_root_requirement_not_satisfied() {
            crate::conditional_tests::solve_conditional_root_requirement_not_satisfied::<$T>();
        }

        #[test]
        fn test_conditional_root_requirement_with_logic() {
            crate::conditional_tests::solve_conditional_root_requirement_with_logic::<$T>();
        }

        #[test]
        fn test_rattler_issue_1917_platform_conditionals() {
            crate::conditional_tests::rattler_issue_1917_platform_conditionals::<$T>();
        }

        #[test]
        fn test_rattler_issue_1917_version_conditionals() {
            crate::conditional_tests::rattler_issue_1917_version_conditionals::<$T>();
        }

        #[test]
        fn test_solve_with_unparsable_dependency() {
            crate::solver_case_tests::solve_with_unparsable_dependency::<$T>();
        }
    };
}

#[cfg(feature = "libsolv_c")]
mod libsolv_c {
    #![allow(unused_imports)] // For some reason windows thinks this is an unused import.

    use rattler_solve::{ChannelPriority, SolveStrategy};

    use super::{
        dummy_channel_json_path, installed_package, solve, solve_real_world, FromStr,
        GenericVirtualPackage, SimpleSolveTask, SolveError, Version,
    };

    solver_backend_tests!(rattler_solve::libsolv_c::Solver);

    #[test]
    #[cfg(target_family = "unix")]
    fn test_solve_with_cached_solv_file_install_new() {
        use rattler_conda_types::{Channel, ChannelConfig, MatchSpec, RepoDataRecord};
        use rattler_solve::{SolverImpl, SolverTask};

        use super::read_repodata;

        let repo_data = read_repodata(&dummy_channel_json_path());

        let cached_repo_data = rattler_solve::libsolv_c::cache_repodata(
            Channel::from_str(
                "conda-forge",
                &ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap()),
            )
            .unwrap()
            .platform_url(rattler_conda_types::Platform::Linux64)
            .to_string(),
            &repo_data,
            None,
        )
        .unwrap();

        let libsolv_repodata = rattler_solve::libsolv_c::RepoData {
            records: repo_data.iter().collect(),
            solv_file: Some(&cached_repo_data),
        };

        let specs: Vec<MatchSpec> = vec!["foo<4".parse().unwrap()];

        let pkgs: Vec<RepoDataRecord> = rattler_solve::libsolv_c::Solver
            .solve(SolverTask {
                locked_packages: Vec::new(),
                virtual_packages: Vec::new(),
                available_packages: [libsolv_repodata],
                specs,
                constraints: Vec::new(),
                pinned_packages: Vec::new(),
                timeout: None,
                channel_priority: ChannelPriority::default(),
                exclude_newer: None,
                min_age: None,
                strategy: SolveStrategy::default(),
            })
            .unwrap()
            .records;

        if pkgs.is_empty() {
            println!("No packages in the environment!");
        }

        assert_eq!(1, pkgs.len());
        let info = &pkgs[0];

        assert_eq!(
            "foo-3.0.2-py36h1af98f8_3.conda",
            info.identifier.to_string()
        );
        assert_eq!(
            "https://conda.anaconda.org/conda-forge/linux-64/foo-3.0.2-py36h1af98f8_3.conda",
            info.url.to_string()
        );
        assert_eq!(
            Some("https://conda.anaconda.org/conda-forge/"),
            info.channel.as_deref()
        );
        assert_eq!("foo", info.package_record.name.as_normalized());
        assert_eq!("linux-64", info.package_record.subdir);
        assert_eq!("3.0.2", info.package_record.version.to_string());
        assert_eq!("py36h1af98f8_3", info.package_record.build);
        assert_eq!(3, info.package_record.build_number);
        assert_eq!(
            rattler_digest::parse_digest_from_hex::<rattler_digest::Sha256>(
                "67a63bec3fd3205170eaad532d487595b8aaceb9814d13c6858d7bac3ef24cd4"
            )
            .as_ref()
            .unwrap(),
            info.package_record.sha256.as_ref().unwrap()
        );
        assert_eq!(
            rattler_digest::parse_digest_from_hex::<rattler_digest::Md5>(
                "fb731d9290f0bcbf3a054665f33ec94f"
            )
            .as_ref()
            .unwrap(),
            info.package_record.md5.as_ref().unwrap()
        );
    }
}

#[cfg(feature = "resolvo")]
mod resolvo {
    use rattler_conda_types::{
        package::DistArchiveIdentifier, MatchSpec, PackageRecord, ParseStrictness, RepoDataRecord,
        VersionWithSource,
    };
    use rattler_solve::{SolveStrategy, SolverImpl, SolverTask};
    use url::Url;

    use super::dummy_channel_with_optional_dependencies_json_path;
    use super::{
        dummy_channel_json_path, installed_package, solve, solve_real_world, FromStr,
        GenericVirtualPackage, SimpleSolveTask, SolveError, Version,
    };

    solver_backend_tests!(rattler_solve::resolvo::Solver);

    #[test]
    fn test_solve_locked() {
        let result = solve::<rattler_solve::resolvo::Solver>(
            &[dummy_channel_json_path()],
            SimpleSolveTask {
                specs: &["bors >=2"],
                pinned_packages: vec![installed_package(
                    "conda-forge",
                    "linux-64",
                    "bors",
                    "1.0",
                    "bla_1",
                    1,
                )],
                ..SimpleSolveTask::default()
            },
        );

        // We expect an error here. `bors` is pinned to 1, but we try to install `>=2`.
        insta::assert_snapshot!(result.unwrap_err());
    }

    #[test]
    fn test_issue_717() {
        let result = solve::<rattler_solve::resolvo::Solver>(
            &[dummy_channel_json_path()],
            SimpleSolveTask {
                specs: &["issue_717"],
                ..SimpleSolveTask::default()
            },
        );

        // We expect an error here. `bors` is pinned to 1, but we try to install `>=2`.
        insta::assert_snapshot!(result.unwrap_err());
    }

    #[test]
    fn test_exclude_newer_error() {
        let date = "2021-12-12T12:12:12Z".parse::<DateTime<Utc>>().unwrap();

        let result = solve::<rattler_solve::resolvo::Solver>(
            &[dummy_channel_json_path()],
            SimpleSolveTask {
                specs: &["foo>=4"],
                exclude_newer: Some(date),
                ..SimpleSolveTask::default()
            },
        );

        // We expect an error here. `bors` is pinnend to 1, but we try to install `>=2`.
        insta::assert_snapshot!(result.unwrap_err());
    }

    /// Try to solve a package with a direct url, and then try to do it again
    /// without having it in the repodata.
    #[test]
    fn test_solve_on_url() {
        let url_str =
            "https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2";
        let url = Url::parse(url_str).unwrap();

        // Create a match spec for a package that is not in the repodata
        let specs: Vec<_> = vec![MatchSpec::from_str(url_str, ParseStrictness::Lenient).unwrap()];

        // Create RepoData with only the package from the url, so the solver can find it
        let package_record = PackageRecord::new(
            // // Only defining the name, version and url is enough for the solver to find the
            // package direct_url: Some(url.clone()),
            "_libgcc_mutex".parse().unwrap(),
            VersionWithSource::from_str("0.1").unwrap(),
            "0".to_string(),
        );
        let repo_data: Vec<RepoDataRecord> = vec![RepoDataRecord {
            package_record: package_record.clone(),
            // Mocking the rest of the fields
            identifier: DistArchiveIdentifier::try_from_url(&url).unwrap(),
            url: url.clone(),
            channel: None,
        }];

        // Completely clean solver task, except for the specs and RepoData
        let task = SolverTask {
            locked_packages: vec![],
            virtual_packages: vec![],
            specs: specs.clone(),
            constraints: vec![],
            pinned_packages: vec![],
            exclude_newer: None,
            strategy: SolveStrategy::default(),
            ..SolverTask::from_iter([&repo_data])
        };

        let pkgs: Vec<RepoDataRecord> = rattler_solve::resolvo::Solver.solve(task).unwrap().records;

        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].package_record.name.as_normalized(), "_libgcc_mutex");
        assert_eq!(pkgs[0].url, url.clone());
        assert_eq!(
            pkgs[0].package_record.version,
            Version::from_str("0.1").unwrap(),
            "expected lowest version of _libgcc_mutex"
        );

        // -----------------------------------------------------------------------------------------
        // Break the url in the repodata, making it not a direct url record.

        let repo_data: Vec<RepoDataRecord> = vec![RepoDataRecord {
            package_record,
            identifier: DistArchiveIdentifier::try_from_url(&url).unwrap(),
            url: Url::from_str("https://false.dont").unwrap(),
            channel: None,
        }];

        // Completely clean solver task, except for the specs and RepoData
        let task = SolverTask {
            locked_packages: vec![],
            virtual_packages: vec![],
            specs,
            constraints: vec![],
            pinned_packages: vec![],
            exclude_newer: None,
            strategy: SolveStrategy::default(),
            ..SolverTask::from_iter([&repo_data])
        };

        let solve_error = rattler_solve::resolvo::Solver.solve(task).unwrap_err();

        assert!(matches!(solve_error, SolveError::Unsolvable(_)));
    }

    #[test]
    fn test_panic_on_constraint() {
        let result = solve::<rattler_solve::resolvo::Solver>(
            &[dummy_channel_json_path()],
            SimpleSolveTask {
                specs: &["xbar"],
                constraints: vec!["xfoo==1"],
                pinned_packages: vec![installed_package(
                    "conda-forge",
                    "linux-64",
                    "xfoo",
                    "1",
                    "xxx",
                    1,
                )],
                ..SimpleSolveTask::default()
            },
        );

        insta::assert_snapshot!(result.unwrap_err());
    }

    /// A test that checks that if two extras have conflicting dependencies the
    /// solution is unsolvable.
    #[test]
    fn test_optional_dependency_conflicting_extras() {
        let result = solve::<rattler_solve::resolvo::Solver>(
            &[dummy_channel_with_optional_dependencies_json_path()],
            SimpleSolveTask {
                specs: &["conflicting-extras[extras=[extra1, extra2]]"],
                ..SimpleSolveTask::default()
            },
        );

        insta::assert_snapshot!(result.unwrap_err(), @r###"
        Cannot solve the request because of: The following packages are incompatible
        ├─ conflicting-extras[extra1] can be installed with any of the following options:
        │  └─ conflicting-extras[extra1]
        ├─ conflicting-extras[extra2] can be installed with any of the following options:
        │  └─ conflicting-extras[extra2]
        └─ conflicting-extras * cannot be installed because there are no viable options:
           └─ conflicting-extras 1 would require
              ├─ bar >=2, which can be installed with any of the following options:
              │  └─ bar 2
              └─ bar <2, which cannot be installed because there are no viable options:
                 └─ bar 1, which conflicts with the versions reported above.
        "###);
    }

    /// A test that checks that extras can cause conflicts with other package
    /// dependencies.
    #[test]
    fn test_optional_dependency_conflicting_with_package() {
        let result = solve::<rattler_solve::resolvo::Solver>(
            &[dummy_channel_with_optional_dependencies_json_path()],
            SimpleSolveTask {
                specs: &["conflicting-extras[extras=[extra1]]", "bar>=2"],
                ..SimpleSolveTask::default()
            },
        );

        insta::assert_snapshot!(result.unwrap_err(), @r###"
        Cannot solve the request because of: The following packages are incompatible
        ├─ conflicting-extras[extra1] can be installed with any of the following options:
        │  └─ conflicting-extras[extra1]
        ├─ bar >=2 can be installed with any of the following options:
        │  └─ bar 2
        └─ conflicting-extras * cannot be installed because there are no viable options:
           └─ conflicting-extras 1 would require
              └─ bar <2, which cannot be installed because there are no viable options:
                 └─ bar 1, which conflicts with the versions reported above.
        "###);
    }

    // Strategy tests (resolvo-specific)

    #[test]
    fn test_lowest_version_strategy() {
        crate::strategy_tests::solve_lowest_version_strategy::<rattler_solve::resolvo::Solver>();
    }

    #[test]
    fn test_lowest_version_strategy_transitive() {
        crate::strategy_tests::solve_lowest_version_strategy_transitive::<
            rattler_solve::resolvo::Solver,
        >();
    }

    #[test]
    fn test_lowest_version_direct_strategy() {
        crate::strategy_tests::solve_lowest_version_direct_strategy::<rattler_solve::resolvo::Solver>(
        );
    }
}

#[derive(Default)]
struct SimpleSolveTask<'a> {
    specs: &'a [&'a str],
    constraints: Vec<&'a str>,
    installed_packages: Vec<RepoDataRecord>,
    pinned_packages: Vec<RepoDataRecord>,
    virtual_packages: Vec<GenericVirtualPackage>,
    exclude_newer: Option<DateTime<Utc>>,
    min_age: Option<MinimumAgeConfig>,
    strategy: SolveStrategy,
}

fn solve<T: SolverImpl + Default>(
    repo_path: &[String],
    task: SimpleSolveTask<'_>,
) -> Result<SolverResult, SolveError> {
    let repo_data = repo_path
        .iter()
        .map(|path| read_repodata(path))
        .collect::<Vec<_>>();

    let specs: Vec<_> = task
        .specs
        .iter()
        .map(|m| {
            MatchSpec::from_str(
                m,
                ParseMatchSpecOptions::lenient().with_experimental_extras(true),
            )
            .unwrap()
        })
        .collect();

    let constraints = task
        .constraints
        .into_iter()
        .map(|m| {
            MatchSpec::from_str(
                m,
                ParseMatchSpecOptions::lenient().with_experimental_extras(true),
            )
            .unwrap()
        })
        .collect();

    let task = SolverTask {
        locked_packages: task.installed_packages,
        virtual_packages: task.virtual_packages,
        specs,
        constraints,
        pinned_packages: task.pinned_packages,
        exclude_newer: task.exclude_newer,
        min_age: task.min_age,
        strategy: task.strategy,
        ..SolverTask::from_iter(&repo_data)
    };

    let pkgs = T::default().solve(task)?;

    if pkgs.records.is_empty() {
        println!("No packages in the environment!");
    }

    Ok(pkgs)
}

#[derive(Default)]
struct CompareTask<'a> {
    specs: Vec<&'a str>,
    exclude_newer: Option<DateTime<Utc>>,
}

fn compare_solve(task: CompareTask<'_>) {
    let specs = task
        .specs
        .iter()
        .map(|s| MatchSpec::from_str(s, ParseStrictness::Lenient).unwrap())
        .collect::<Vec<_>>();

    let sparse_repo_data = read_real_world_repo_data();

    let names = specs.iter().filter_map(|s| {
        s.name
            .as_ref()
            .and_then(|n| Option::<PackageName>::from(n.clone()))
    });
    let available_packages = SparseRepoData::load_records_recursive(
        sparse_repo_data,
        names,
        None,
        PackageFormatSelection::default(),
    )
    .unwrap();

    let extract_pkgs = |records: Vec<RepoDataRecord>| {
        let mut pkgs = records
            .into_iter()
            .map(|pkg| {
                format!(
                    "{} {} {}",
                    pkg.package_record.name.as_normalized(),
                    pkg.package_record.version,
                    pkg.package_record.build
                )
            })
            .collect::<Vec<_>>();

        // The order of packages is nondeterministic, so we sort them to ensure we can
        // compare them to a previous run
        pkgs.sort();
        pkgs
    };

    let mut results = Vec::new();

    #[cfg(feature = "libsolv_c")]
    {
        let start_solve = Instant::now();
        results.push((
            "libsolv_c",
            extract_pkgs(
                rattler_solve::libsolv_c::Solver
                    .solve(SolverTask {
                        specs: specs.clone(),
                        exclude_newer: task.exclude_newer,
                        ..SolverTask::from_iter(&available_packages)
                    })
                    .unwrap()
                    .records,
            ),
        ));
        let end_solve = Instant::now();
        println!("libsolv_c took {}ms", (end_solve - start_solve).as_millis());
    }

    #[cfg(feature = "resolvo")]
    {
        let start_solve = Instant::now();
        results.push((
            "resolvo",
            extract_pkgs(
                rattler_solve::resolvo::Solver
                    .solve(SolverTask {
                        specs: specs.clone(),
                        exclude_newer: task.exclude_newer,
                        ..SolverTask::from_iter(&available_packages)
                    })
                    .unwrap()
                    .records,
            ),
        ));
        let end_solve = Instant::now();
        println!("resolvo took {}ms", (end_solve - start_solve).as_millis());
    }

    results.into_iter().fold(None, |previous, current| {
        let previous = match previous {
            Some(previous) => previous,
            None => return Some(current),
        };

        similar_asserts::assert_eq!(
            &previous.1,
            &current.1,
            "The result between {} and {} differs",
            &previous.0,
            &current.0
        );

        Some(current)
    });
}

#[test]
fn compare_solve_tensorboard() {
    compare_solve(CompareTask {
        specs: vec!["tensorboard=2.1.1", "grpc-cpp=1.39.1"],
        ..CompareTask::default()
    });
}

#[test]
fn compare_solve_python() {
    compare_solve(CompareTask {
        specs: vec!["python=3.9"],
        ..CompareTask::default()
    });
}

#[test]
fn compare_solve_tensorflow() {
    compare_solve(CompareTask {
        specs: vec!["tensorflow"],
        ..CompareTask::default()
    });
}

#[test]
fn compare_solve_quetz() {
    compare_solve(CompareTask {
        specs: vec!["quetz"],
        ..CompareTask::default()
    });
}

#[test]
fn compare_solve_xtensor_xsimd() {
    compare_solve(CompareTask {
        specs: vec!["xtensor", "xsimd"],
        ..CompareTask::default()
    });
}

fn solve_to_get_channel_of_spec<T: SolverImpl + Default>(
    spec_str: &str,
    expected_channel: &str,
    repo_data: Vec<&SparseRepoData>,
    channel_priority: ChannelPriority,
) {
    let spec = MatchSpec::from_str(spec_str, ParseStrictness::Lenient).unwrap();
    let specs = vec![spec.clone()];
    let names = specs.iter().filter_map(|s| {
        s.name
            .as_ref()
            .and_then(|n| Option::<PackageName>::from(n.clone()))
    });

    let available_packages = SparseRepoData::load_records_recursive(
        repo_data,
        names,
        None,
        PackageFormatSelection::default(),
    )
    .unwrap();

    let task = SolverTask {
        specs: specs.clone(),
        channel_priority,
        ..SolverTask::from_iter(&available_packages)
    };

    let result: Vec<RepoDataRecord> = T::default().solve(task).unwrap().records;

    let record = result.iter().find(|record| {
        spec.name
            .as_ref()
            .unwrap()
            .matches(&record.package_record.name)
    });
    assert_eq!(record.unwrap().channel, Some(expected_channel.to_string()));
}

#[test]
fn channel_specific_requirement() {
    let repodata = vec![
        read_conda_forge_sparse_repo_data(),
        read_pytorch_sparse_repo_data(),
    ];
    solve_to_get_channel_of_spec::<rattler_solve::resolvo::Solver>(
        "conda-forge::pytorch-cpu",
        "https://conda.anaconda.org/conda-forge/",
        repodata.clone(),
        ChannelPriority::Strict,
    );
    solve_to_get_channel_of_spec::<rattler_solve::resolvo::Solver>(
        "conda-forge::pytorch-cpu",
        "https://conda.anaconda.org/conda-forge/",
        repodata.clone(),
        ChannelPriority::Disabled,
    );
    solve_to_get_channel_of_spec::<rattler_solve::resolvo::Solver>(
        "pytorch::pytorch-cpu",
        "https://conda.anaconda.org/pytorch/",
        repodata.clone(),
        ChannelPriority::Strict,
    );
    solve_to_get_channel_of_spec::<rattler_solve::resolvo::Solver>(
        "pytorch::pytorch-cpu",
        "https://conda.anaconda.org/pytorch/",
        repodata,
        ChannelPriority::Disabled,
    );
}

#[test]
fn channel_priority_strict() {
    // Solve with conda-forge as the first channel
    let repodata = vec![
        read_conda_forge_sparse_repo_data(),
        read_pytorch_sparse_repo_data(),
    ];
    solve_to_get_channel_of_spec::<rattler_solve::resolvo::Solver>(
        "pytorch-cpu",
        "https://conda.anaconda.org/conda-forge/",
        repodata,
        ChannelPriority::Strict,
    );

    // Solve with pytorch as the first channel
    let repodata = vec![
        read_pytorch_sparse_repo_data(),
        read_conda_forge_sparse_repo_data(),
    ];
    solve_to_get_channel_of_spec::<rattler_solve::resolvo::Solver>(
        "pytorch-cpu",
        "https://conda.anaconda.org/pytorch/",
        repodata,
        ChannelPriority::Strict,
    );
}

#[test]
#[should_panic(
    expected = "called `Result::unwrap()` on an `Err` value: Unsolvable([\"The following packages \
    are incompatible\\n└─ pytorch-cpu ==0.4.1 py36_cpu_1 cannot be installed because there are no \
    viable options:\\n   └─ pytorch-cpu 0.4.1 is excluded because due to strict channel priority\\n  \
    available from: https://conda.anaconda.org/pytorch/\\n  but blocked by higher-priority channel: \
    https://conda.anaconda.org/conda-forge/\\n\"])"
)]
fn channel_priority_strict_panic() {
    let repodata = vec![
        read_conda_forge_sparse_repo_data(),
        read_pytorch_sparse_repo_data(),
    ];
    solve_to_get_channel_of_spec::<rattler_solve::resolvo::Solver>(
        "pytorch-cpu=0.4.1=py36_cpu_1",
        "https://conda.anaconda.org/pytorch/",
        repodata,
        ChannelPriority::Strict,
    );
}

#[test]
fn channel_priority_disabled() {
    let repodata = vec![
        read_conda_forge_sparse_repo_data(),
        read_pytorch_sparse_repo_data(),
    ];
    solve_to_get_channel_of_spec::<rattler_solve::resolvo::Solver>(
        "pytorch-cpu=0.4.1=py36_cpu_1",
        "https://conda.anaconda.org/pytorch/",
        repodata,
        ChannelPriority::Disabled,
    );
}

#[cfg(feature = "libsolv_c")]
#[test]
#[should_panic(
    expected = "called `Result::unwrap()` on an `Err` value: Unsolvable([\"package \
    pytorch-cpu-0.4.1-py36_cpu_1 is excluded by strict repo priority\"])"
)]
fn channel_priority_strict_libsolv_c() {
    let repodata = vec![
        read_conda_forge_sparse_repo_data(),
        read_pytorch_sparse_repo_data(),
    ];

    solve_to_get_channel_of_spec::<rattler_solve::libsolv_c::Solver>(
        "pytorch-cpu=0.4.1=py36_cpu_1",
        "https://conda.anaconda.org/pytorch/",
        repodata,
        ChannelPriority::Strict,
    );
}

#[cfg(feature = "libsolv_c")]
#[test]
fn channel_priority_disabled_libsolv_c() {
    let repodata = vec![
        read_conda_forge_sparse_repo_data(),
        read_pytorch_sparse_repo_data(),
    ];

    solve_to_get_channel_of_spec::<rattler_solve::libsolv_c::Solver>(
        "pytorch-cpu=0.4.1=py36_cpu_1",
        "https://conda.anaconda.org/pytorch/",
        repodata,
        ChannelPriority::Disabled,
    );
}
