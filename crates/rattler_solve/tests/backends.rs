use std::{str::FromStr, time::Instant};

use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use rattler_conda_types::{
    Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, NoArchType, PackageRecord,
    ParseStrictness, RepoData, RepoDataRecord, Version,
};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{ChannelPriority, SolveError, SolveStrategy, SolverImpl, SolverTask};
use url::Url;

fn channel_config() -> ChannelConfig {
    ChannelConfig::default_with_root_dir(std::env::current_dir().unwrap())
}

fn conda_json_path() -> String {
    format!(
        "{}/{}",
        env!("CARGO_MANIFEST_DIR"),
        "../../test-data/channels/conda-forge/linux-64/repodata.json"
    )
}

fn conda_json_path_noarch() -> String {
    format!(
        "{}/{}",
        env!("CARGO_MANIFEST_DIR"),
        "../../test-data/channels/conda-forge/noarch/repodata.json"
    )
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

fn dummy_md5_hash() -> rattler_digest::Md5Hash {
    rattler_digest::parse_digest_from_hex::<rattler_digest::Md5>("b3af409bb8423187c75e6c7f5b683908")
        .unwrap()
}

fn dummy_sha256_hash() -> rattler_digest::Sha256Hash {
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
    SparseRepoData::new(
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
    RepoDataRecord {
        url: Url::from_str("http://example.com").unwrap(),
        channel: channel.to_string(),
        file_name: "dummy-filename".to_string(),
        package_record: PackageRecord {
            name: name.parse().unwrap(),
            version: version.parse().unwrap(),
            build: build.to_string(),
            build_number,
            subdir: subdir.to_string(),
            md5: Some(dummy_md5_hash()),
            sha256: Some(dummy_sha256_hash()),
            size: None,
            arch: None,
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
    }
}

fn solve_real_world<T: SolverImpl + Default>(specs: Vec<&str>) -> Vec<String> {
    let specs = specs
        .iter()
        .map(|s| MatchSpec::from_str(s, ParseStrictness::Lenient).unwrap())
        .collect::<Vec<_>>();

    let sparse_repo_data = read_real_world_repo_data();

    let names = specs.iter().filter_map(|s| s.name.as_ref().cloned());
    let available_packages =
        SparseRepoData::load_records_recursive(sparse_repo_data, names, None).unwrap();

    let solver_task = SolverTask {
        specs: specs.clone(),
        ..SolverTask::from_iter(&available_packages)
    };

    let pkgs1 = match T::default().solve(solver_task) {
        Ok(result) => result,
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
        let json_file = conda_json_path();
        let json_file_noarch = conda_json_path_noarch();

        vec![
            read_sparse_repodata(&json_file),
            read_sparse_repodata(&json_file_noarch),
        ]
    });

    &REPO_DATA
}

fn read_pytorch_sparse_repo_data() -> &'static SparseRepoData {
    static REPO_DATA: Lazy<SparseRepoData> = Lazy::new(|| {
        let pytorch = pytorch_json_path();
        SparseRepoData::new(
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
        let conda_forge = conda_json_path();
        SparseRepoData::new(
            Channel::from_str("conda-forge", &channel_config()).unwrap(),
            "conda-forge".to_string(),
            conda_forge,
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
        fn test_solve_favored() {
            let result = solve::<$T>(
                &[dummy_channel_json_path()],
                SimpleSolveTask {
                    specs: &["bors"],
                    installed_packages: vec![installed_package(
                        "conda-forge",
                        "linux-64",
                        "bors",
                        "1.0",
                        "bla_1",
                        1,
                    )],
                    ..SimpleSolveTask::default()
                },
            )
            .unwrap();

            assert_eq!(result.len(), 1);
            assert_eq!(result[0].package_record.to_string(), "bors=1.0=bla_1");
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

            assert_eq!(pkgs.len(), 1);

            let info = &pkgs[0];
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

            assert_eq!(1, pkgs.len());
            let info = &pkgs[0];

            assert_eq!("foo-3.0.2-py36h1af98f8_3.conda", info.file_name);
            assert_eq!(
                "https://conda.anaconda.org/conda-forge/linux-64/foo-3.0.2-py36h1af98f8_3.conda",
                info.url.to_string()
            );
            assert_eq!("https://conda.anaconda.org/conda-forge/", info.channel);
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
            assert_eq!(operations.len(), 1);
            assert_eq!(operations[0].file_name, "foo-3.0.2-py36h1af98f8_1.conda");
        }

        #[test]
        fn test_solve_dummy_repo_install_noop() {
            let already_installed = vec![installed_package(
                "conda-forge",
                "linux-64",
                "foo",
                "3.0.2",
                "py36h1af98f8_1",
                1,
            )];

            let pkgs = solve::<$T>(
                &[dummy_channel_json_path()],
                SimpleSolveTask {
                    specs: &["foo<4"],
                    installed_packages: already_installed,
                    ..SimpleSolveTask::default()
                },
            )
            .unwrap();

            assert_eq!(1, pkgs.len());

            // Install
            let info = &pkgs[0];
            assert_eq!("foo", info.package_record.name.as_normalized());
            assert_eq!("3.0.2", &info.package_record.version.to_string());
        }

        #[test]
        fn test_solve_dummy_repo_upgrade() {
            let already_installed = vec![installed_package(
                "conda-forge",
                "linux-64",
                "foo",
                "3.0.2",
                "py36h1af98f8_1",
                1,
            )];

            let pkgs = solve::<$T>(
                &[dummy_channel_json_path()],
                SimpleSolveTask {
                    specs: &["foo>=4"],
                    installed_packages: already_installed,
                    ..SimpleSolveTask::default()
                },
            )
            .unwrap();

            // Install
            let info = &pkgs[0];
            assert_eq!("foo", info.package_record.name.as_normalized());
            assert_eq!("4.0.2", &info.package_record.version.to_string());
        }

        #[test]
        fn test_solve_dummy_repo_downgrade() {
            let already_installed = vec![installed_package(
                "conda-forge",
                "linux-64",
                "foo",
                "4.0.2",
                "py36h1af98f8_1",
                1,
            )];

            let pkgs = solve::<$T>(
                &[dummy_channel_json_path()],
                SimpleSolveTask {
                    specs: &["foo<4"],
                    installed_packages: already_installed,
                    ..SimpleSolveTask::default()
                },
            )
            .unwrap();

            assert_eq!(pkgs.len(), 1);

            // Uninstall
            let info = &pkgs[0];
            assert_eq!("foo", info.package_record.name.as_normalized());
            assert_eq!("3.0.2", &info.package_record.version.to_string());
        }

        #[test]
        fn test_solve_dummy_repo_remove() {
            let already_installed = vec![installed_package(
                "conda-forge",
                "linux-64",
                "foo",
                "3.0.2",
                "py36h1af98f8_1",
                1,
            )];

            let pkgs = solve::<$T>(
                &[dummy_channel_json_path()],
                SimpleSolveTask {
                    installed_packages: already_installed,
                    ..SimpleSolveTask::default()
                },
            )
            .unwrap();

            // Should be no packages!
            assert_eq!(0, pkgs.len());
        }

        #[test]
        fn test_exclude_newer() {
            let date = "2021-12-12T12:12:12Z".parse::<DateTime<Utc>>().unwrap();

            let pkgs = solve::<$T>(
                &[dummy_channel_json_path()],
                SimpleSolveTask {
                    specs: &["foo"],
                    exclude_newer: Some(date),
                    ..SimpleSolveTask::default()
                },
            )
            .unwrap();

            assert_eq!(1, pkgs.len());

            let info = &pkgs[0];
            assert_eq!("foo", info.package_record.name.as_normalized());
            assert_eq!("3.0.2", &info.package_record.version.to_string(),
                "although there is a newer version available we expect an older version of foo because we exclude the newer version based on the timestamp");
            assert_eq!(&info.file_name, "foo-3.0.2-py36h1af98f8_1.tar.bz2", "even though there is a conda version available we expect the tar.bz2 version because we exclude the .conda version based on the timestamp");
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
        fn test_constraints() {
            // There following package is provided as .tar.bz and as .conda in repodata.json
            let mut operations = solve::<$T>(
                &[dummy_channel_json_path()],
                SimpleSolveTask {
                    specs: &["foobar"],
                    constraints: vec!["bors <=1", "nonexisting"],
                    ..SimpleSolveTask::default()
                },
            )
            .unwrap();

            // Sort operations by file name to make the test deterministic
            operations.sort_by(|a, b| a.file_name.cmp(&b.file_name));

            assert_eq!(operations.len(), 2);
            assert_eq!(operations[0].file_name, "bors-1.0-bla_1.tar.bz2");
            assert_eq!(operations[1].file_name, "foobar-2.1-bla_1.tar.bz2");
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
        use rattler_conda_types::{Channel, ChannelConfig, MatchSpec};
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

        let pkgs = rattler_solve::libsolv_c::Solver
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
                strategy: SolveStrategy::default(),
            })
            .unwrap();

        if pkgs.is_empty() {
            println!("No packages in the environment!");
        }

        assert_eq!(1, pkgs.len());
        let info = &pkgs[0];

        assert_eq!("foo-3.0.2-py36h1af98f8_3.conda", info.file_name);
        assert_eq!(
            "https://conda.anaconda.org/conda-forge/linux-64/foo-3.0.2-py36h1af98f8_3.conda",
            info.url.to_string()
        );
        assert_eq!("https://conda.anaconda.org/conda-forge/", info.channel);
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
        MatchSpec, PackageRecord, ParseStrictness, RepoDataRecord, VersionWithSource,
    };
    use rattler_solve::{SolveStrategy, SolverImpl, SolverTask};
    use url::Url;

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

        // We expect an error here. `bors` is pinnend to 1, but we try to install `>=2`.
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

        // We expect an error here. `bors` is pinnend to 1, but we try to install `>=2`.
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

    #[test]
    fn test_lowest_version_strategy_highest_build_number() {
        let result = solve::<rattler_solve::resolvo::Solver>(
            &[dummy_channel_json_path()],
            SimpleSolveTask {
                specs: &["foo"],
                strategy: rattler_solve::SolveStrategy::LowestVersion,
                ..SimpleSolveTask::default()
            },
        )
        .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].package_record.version,
            Version::from_str("3.0.2").unwrap()
        );
        assert_eq!(
            result[0].package_record.build_number, 3,
            "expected the highest build number"
        );
    }

    #[test]
    fn test_lowest_version_strategy_all() {
        let result = solve::<rattler_solve::resolvo::Solver>(
            &[dummy_channel_json_path()],
            SimpleSolveTask {
                specs: &["foobar"],
                strategy: rattler_solve::SolveStrategy::LowestVersion,
                ..SimpleSolveTask::default()
            },
        )
        .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].package_record.name.as_normalized(), "foobar");
        assert_eq!(
            result[0].package_record.version,
            Version::from_str("2.0").unwrap(),
            "expected lowest version of foobar"
        );

        assert_eq!(result[1].package_record.name.as_normalized(), "bors");
        assert_eq!(
            result[1].package_record.version,
            Version::from_str("1.0").unwrap(),
            "expected lowest version of bors"
        );
    }

    #[test]
    fn test_lowest_direct_version_strategy() {
        let result = solve::<rattler_solve::resolvo::Solver>(
            &[dummy_channel_json_path()],
            SimpleSolveTask {
                specs: &["foobar"],
                strategy: rattler_solve::SolveStrategy::LowestVersionDirect,
                ..SimpleSolveTask::default()
            },
        )
        .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].package_record.name.as_normalized(), "foobar");
        assert_eq!(
            result[0].package_record.version,
            Version::from_str("2.0").unwrap(),
            "expected lowest version of foobar"
        );

        assert_eq!(result[1].package_record.name.as_normalized(), "bors");
        assert_eq!(
            result[1].package_record.version,
            Version::from_str("1.2.1").unwrap(),
            "expected highest compatible version of bors"
        );
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
            file_name: url_str.to_string(),
            url: url.clone(),
            channel: "".to_string(),
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

        let pkgs = rattler_solve::resolvo::Solver.solve(task).unwrap();

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
            file_name: url_str.to_string(),
            url: Url::from_str("https://false.dont").unwrap(),
            channel: "".to_string(),
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
}

#[derive(Default)]
struct SimpleSolveTask<'a> {
    specs: &'a [&'a str],
    constraints: Vec<&'a str>,
    installed_packages: Vec<RepoDataRecord>,
    pinned_packages: Vec<RepoDataRecord>,
    virtual_packages: Vec<GenericVirtualPackage>,
    exclude_newer: Option<DateTime<Utc>>,
    strategy: SolveStrategy,
}

fn solve<T: SolverImpl + Default>(
    repo_path: &[String],
    task: SimpleSolveTask<'_>,
) -> Result<Vec<RepoDataRecord>, SolveError> {
    let repo_data = repo_path
        .iter()
        .map(|path| read_repodata(path))
        .collect::<Vec<_>>();

    let specs: Vec<_> = task
        .specs
        .iter()
        .map(|m| MatchSpec::from_str(m, ParseStrictness::Lenient).unwrap())
        .collect();

    let constraints = task
        .constraints
        .into_iter()
        .map(|m| MatchSpec::from_str(m, ParseStrictness::Lenient).unwrap())
        .collect();

    let task = SolverTask {
        locked_packages: task.installed_packages,
        virtual_packages: task.virtual_packages,
        specs,
        constraints,
        pinned_packages: task.pinned_packages,
        exclude_newer: task.exclude_newer,
        strategy: task.strategy,
        ..SolverTask::from_iter(&repo_data)
    };

    let pkgs = T::default().solve(task)?;

    if pkgs.is_empty() {
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

    let names = specs.iter().filter_map(|s| s.name.as_ref().cloned());
    let available_packages =
        SparseRepoData::load_records_recursive(sparse_repo_data, names, None).unwrap();

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
                    .unwrap(),
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
                    .unwrap(),
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
    let names = specs.iter().filter_map(|s| s.name.as_ref().cloned());

    let available_packages =
        SparseRepoData::load_records_recursive(repo_data, names, None).unwrap();

    let task = SolverTask {
        specs: specs.clone(),
        channel_priority,
        ..SolverTask::from_iter(&available_packages)
    };

    let result = T::default().solve(task).unwrap();

    let record = result.iter().find(|record| {
        record.package_record.name.as_normalized() == spec.name.as_ref().unwrap().as_normalized()
    });
    assert_eq!(record.unwrap().channel, expected_channel.to_string());
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
    viable options:\\n   └─ pytorch-cpu 0.4.1 is excluded because due to strict channel priority \
    not using this option from: 'https://conda.anaconda.org/pytorch/'\\n\"])"
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
