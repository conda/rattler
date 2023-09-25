use once_cell::sync::Lazy;
use rattler_conda_types::{
    Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, NoArchType, PackageRecord, RepoData,
    RepoDataRecord, Version,
};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{SolveError, SolverImpl, SolverTask};
use std::str::FromStr;
use std::time::Instant;
use url::Url;

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
    repo_data.into_repo_data_records(
        &Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap(),
    )
}

fn read_sparse_repodata(path: &str) -> SparseRepoData {
    SparseRepoData::new(
        Channel::from_str("dummy", &ChannelConfig::default()).unwrap(),
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
        },
    }
}

fn solve_real_world<T: SolverImpl + Default>(specs: Vec<&str>) -> Vec<String> {
    let specs = specs
        .iter()
        .map(|s| MatchSpec::from_str(s).unwrap())
        .collect::<Vec<_>>();

    let sparse_repo_datas = read_real_world_repo_data();

    let names = specs.iter().filter_map(|s| s.name.as_ref().cloned());
    let available_packages =
        SparseRepoData::load_records_recursive(sparse_repo_datas, names, None).unwrap();

    let solver_task = SolverTask {
        available_packages: &available_packages,
        specs: specs.clone(),
        locked_packages: Default::default(),
        pinned_packages: Default::default(),
        virtual_packages: Default::default(),
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

        // The order of packages is nondeterministic, so we sort them to ensure we can compare them
        // to a previous run
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

macro_rules! solver_backend_tests {
    ($T:path) => {
        #[test]
        fn test_solve_quetz() {
            insta::assert_yaml_snapshot!(solve_real_world::<$T>(vec!["quetz",]));
        }

        #[test]
        fn test_solve_xtensor_xsimd() {
            insta::assert_yaml_snapshot!(solve_real_world::<$T>(vec!["xtensor", "xsimd",]));
        }

        #[test_log::test]
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
        fn test_solve_favored() {
            let result = solve::<$T>(
                dummy_channel_json_path(),
                vec![installed_package(
                    "conda-forge",
                    "linux-64",
                    "bors",
                    "1.0",
                    "bla_1",
                    1,
                )],
                Vec::new(),
                Vec::new(),
                &["bors"],
            )
            .unwrap();

            assert_eq!(result.len(), 1);
            assert_eq!(result[0].package_record.to_string(), "bors=1.0=bla_1");
        }

        #[test]
        fn test_solve_with_error() {
            let result = solve::<$T>(
                dummy_channel_json_path(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                &["foobar >=2", "bors >= 2"],
            );

            assert!(result.is_err());

            let err = result.err().unwrap();
            insta::assert_display_snapshot!(err);
        }

        #[test]
        fn test_solve_dummy_repo_install_non_existent() {
            let result = solve::<$T>(
                dummy_channel_json_path(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                &["asdfasdf", "foo<4"],
            );

            assert!(result.is_err());

            let err = result.err().unwrap();
            insta::assert_debug_snapshot!(err);
        }

        #[test]
        fn test_solve_dummy_repo_missing_virtual_package() {
            let result = solve::<$T>(
                dummy_channel_json_path(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                &["bar"],
            );

            assert!(matches!(result.err(), Some(SolveError::Unsolvable(_))));
        }

        #[test]
        fn test_solve_dummy_repo_with_virtual_package() {
            let pkgs = solve::<$T>(
                dummy_channel_json_path(),
                Vec::new(),
                Vec::new(),
                vec![GenericVirtualPackage {
                    name: rattler_conda_types::PackageName::new_unchecked("__unix"),
                    version: Version::from_str("0").unwrap(),
                    build_string: "0".to_string(),
                }],
                &["bar"],
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
                dummy_channel_json_path(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                &["foo<4"],
            )
            .unwrap();

            assert_eq!(1, pkgs.len());
            let info = &pkgs[0];

            assert_eq!("foo-3.0.2-py36h1af98f8_1.conda", info.file_name);
            assert_eq!(
                "https://conda.anaconda.org/conda-forge/linux-64/foo-3.0.2-py36h1af98f8_1.conda",
                info.url.to_string()
            );
            assert_eq!("https://conda.anaconda.org/conda-forge/", info.channel);
            assert_eq!("foo", info.package_record.name.as_normalized());
            assert_eq!("linux-64", info.package_record.subdir);
            assert_eq!("3.0.2", info.package_record.version.to_string());
            assert_eq!("py36h1af98f8_1", info.package_record.build);
            assert_eq!(1, info.package_record.build_number);
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
                dummy_channel_json_path(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                &[match_spec],
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
                dummy_channel_json_path(),
                already_installed,
                Vec::new(),
                Vec::new(),
                &["foo<4"],
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
                dummy_channel_json_path(),
                already_installed,
                Vec::new(),
                Vec::new(),
                &["foo>=4"],
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
                dummy_channel_json_path(),
                already_installed,
                Vec::new(),
                Vec::new(),
                &["foo<4"],
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
                dummy_channel_json_path(),
                already_installed,
                Vec::new(),
                Vec::new(),
                &[],
            )
            .unwrap();

            // Should be no packages!
            assert_eq!(0, pkgs.len());
        }
    };
}

#[cfg(feature = "libsolv_c")]
mod libsolv_c {
    use super::*;

    solver_backend_tests!(rattler_solve::libsolv_c::Solver);

    #[test]
    #[cfg(target_family = "unix")]
    fn test_solve_with_cached_solv_file_install_new() {
        let repo_data = read_repodata(&dummy_channel_json_path());

        let cached_repo_data = rattler_solve::libsolv_c::cache_repodata(
            Channel::from_str("conda-forge", &ChannelConfig::default())
                .unwrap()
                .platform_url(rattler_conda_types::Platform::Linux64)
                .to_string(),
            &repo_data,
        );

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
                pinned_packages: Vec::new(),
            })
            .unwrap();

        if pkgs.is_empty() {
            println!("No packages in the environment!");
        }

        assert_eq!(1, pkgs.len());
        let info = &pkgs[0];

        assert_eq!("foo-3.0.2-py36h1af98f8_1.conda", info.file_name);
        assert_eq!(
            "https://conda.anaconda.org/conda-forge/linux-64/foo-3.0.2-py36h1af98f8_1.conda",
            info.url.to_string()
        );
        assert_eq!("https://conda.anaconda.org/conda-forge/", info.channel);
        assert_eq!("foo", info.package_record.name.as_normalized());
        assert_eq!("linux-64", info.package_record.subdir);
        assert_eq!("3.0.2", info.package_record.version.to_string());
        assert_eq!("py36h1af98f8_1", info.package_record.build);
        assert_eq!(1, info.package_record.build_number);
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
    use super::*;

    solver_backend_tests!(rattler_solve::resolvo::Solver);

    #[test]
    fn test_solve_locked() {
        let result = solve::<rattler_solve::resolvo::Solver>(
            dummy_channel_json_path(),
            Vec::new(),
            vec![installed_package(
                "conda-forge",
                "linux-64",
                "bors",
                "1.0",
                "bla_1",
                1,
            )],
            Vec::new(),
            &["bors >=2"],
        );

        // We expect an error here. `bors` is pinnend to 1, but we try to install `>=2`.
        insta::assert_display_snapshot!(result.unwrap_err());
    }
}

fn solve<T: SolverImpl + Default>(
    repo_path: String,
    installed_packages: Vec<RepoDataRecord>,
    pinned_packages: Vec<RepoDataRecord>,
    virtual_packages: Vec<GenericVirtualPackage>,
    match_specs: &[&str],
) -> Result<Vec<RepoDataRecord>, SolveError> {
    let repo_data = read_repodata(&repo_path);

    let specs: Vec<_> = match_specs
        .iter()
        .map(|m| MatchSpec::from_str(m).unwrap())
        .collect();

    let task = SolverTask {
        locked_packages: installed_packages,
        virtual_packages,
        available_packages: [&repo_data],
        specs,
        pinned_packages,
    };

    let pkgs = T::default().solve(task)?;

    if pkgs.is_empty() {
        println!("No packages in the environment!");
    }

    Ok(pkgs)
}

fn compare_solve(specs: Vec<&str>) {
    let specs = specs
        .iter()
        .map(|s| MatchSpec::from_str(s).unwrap())
        .collect::<Vec<_>>();

    let sparse_repo_datas = read_real_world_repo_data();

    let names = specs.iter().filter_map(|s| s.name.as_ref().cloned());
    let available_packages =
        SparseRepoData::load_records_recursive(sparse_repo_datas, names, None).unwrap();

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

        // The order of packages is nondeterministic, so we sort them to ensure we can compare them
        // to a previous run
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
                        available_packages: &available_packages,
                        specs: specs.clone(),
                        locked_packages: Default::default(),
                        pinned_packages: Default::default(),
                        virtual_packages: Default::default(),
                    })
                    .unwrap(),
            ),
        ));
        let end_solve = Instant::now();
        println!("libsolv_c took {}ms", (end_solve - start_solve).as_millis())
    }

    #[cfg(feature = "resolvo")]
    {
        let start_solve = Instant::now();
        results.push((
            "resolvo",
            extract_pkgs(
                rattler_solve::resolvo::Solver
                    .solve(SolverTask {
                        available_packages: &available_packages,
                        specs: specs.clone(),
                        locked_packages: Default::default(),
                        pinned_packages: Default::default(),
                        virtual_packages: Default::default(),
                    })
                    .unwrap(),
            ),
        ));
        let end_solve = Instant::now();
        println!("resolvo took {}ms", (end_solve - start_solve).as_millis())
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
    compare_solve(vec!["tensorboard=2.1.1", "grpc-cpp=1.39.1"]);
}

#[test]
fn compare_solve_python() {
    compare_solve(vec!["python=3.9"]);
}

#[test]
fn compare_solve_tensorflow() {
    compare_solve(vec!["tensorflow"]);
}

#[test]
fn compare_solve_quetz() {
    compare_solve(vec!["quetz"]);
}

#[test]
fn compare_solve_xtensor_xsimd() {
    compare_solve(vec!["xtensor", "xsimd"]);
}
