#![deny(missing_docs)]

//! `rattler_solve` is a crate that provides functionality to solve Conda environments. It currently
//! exposes the functionality through the [`SolverBackend::solve`] function.

mod libsolv;
mod solver_backend;

pub use libsolv::{
    cache_repodata as cache_libsolv_repodata, LibcByteSlice, LibsolvBackend, LibsolvRepoData,
};
pub use solver_backend::SolverBackend;
use std::ffi::NulError;

use rattler_conda_types::GenericVirtualPackage;
use rattler_conda_types::{MatchSpec, RepoDataRecord};

/// Represents an error when solving the dependencies for a given environment
#[derive(thiserror::Error, Debug)]
pub enum SolveError {
    /// There is no set of dependencies that satisfies the requirements
    #[error("unsolvable")]
    Unsolvable,

    /// An error occurred when trying to load the channel and platform's `repodata.json`
    #[error("error adding repodata: {0}")]
    ErrorAddingRepodata(#[source] NulError),

    /// An error occurred when trying to load information about installed packages to the solver
    #[error("error adding installed packages: {0}")]
    ErrorAddingInstalledPackages(#[source] NulError),

    /// The solver backend returned operations that we dont know how to install.
    /// Each string is a somewhat user-friendly representation of which operation was not recognized
    /// and can be used for error reporting
    #[error("unsupported operations")]
    UnsupportedOperations(Vec<String>),
}

/// Represents a dependency resolution problem, to be solved by one of the backends (currently only
/// libsolv is supported)
pub struct SolverProblem<TAvailablePackagesIterator> {
    /// An iterator over all available packages
    pub available_packages: TAvailablePackagesIterator,

    /// Records of packages that are previously selected.
    ///
    /// If the solver encounters multiple variants of a single package (identified by its name), it
    /// will sort the records and select the best possible version. However, if there exists a
    /// locked version it will prefer that variant instead. This is useful to reduce the number of
    /// packages that are updated when installing new packages.
    ///
    /// Usually you add the currently installed packages or packages from a lock-file here.
    pub locked_packages: Vec<RepoDataRecord>,

    /// Records of packages that are previously selected and CANNOT be changed.
    ///
    /// If the solver encounters multiple variants of a single package (identified by its name), it
    /// will sort the records and select the best possible version. However, if there is a variant
    /// available in the `pinned_packages` field it will always select that version no matter what
    /// even if that means other packages have to be downgraded.
    pub pinned_packages: Vec<RepoDataRecord>,

    /// Virtual packages considered active
    pub virtual_packages: Vec<GenericVirtualPackage>,

    /// The specs we want to solve
    pub specs: Vec<MatchSpec>,
}

#[cfg(test)]
mod test_libsolv {
    use crate::libsolv::LibsolvBackend;
    use crate::{LibsolvRepoData, SolveError, SolverBackend, SolverProblem};
    use rattler_conda_types::{
        Channel, ChannelConfig, MatchSpec, NoArchType, PackageRecord, RepoData, RepoDataRecord,
        Version,
    };
    use rattler_conda_types::{GenericVirtualPackage, Platform};
    use std::str::FromStr;
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

    fn dummy_md5_hash() -> &'static str {
        "b3af409bb8423187c75e6c7f5b683908"
    }

    fn dummy_sha256_hash() -> &'static str {
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    }

    fn read_repodata(path: &str) -> Vec<RepoDataRecord> {
        let repo_data: RepoData =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        repo_data.into_repo_data_records(
            &Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap(),
        )
    }

    fn installed_package(
        channel: &str,
        subdir: &str,
        name: &str,
        version: &str,
        build: &str,
        build_number: usize,
    ) -> RepoDataRecord {
        RepoDataRecord {
            url: Url::from_str("http://example.com").unwrap(),
            channel: channel.to_string(),
            file_name: "dummy-filename".to_string(),
            package_record: PackageRecord {
                name: name.to_string(),
                version: Version::from_str(version).unwrap(),
                build: build.to_string(),
                build_number,
                subdir: subdir.to_string(),
                md5: Some(dummy_md5_hash().to_string()),
                sha256: Some(dummy_sha256_hash().to_string()),
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
            },
        }
    }

    #[test]
    fn test_solve_python() {
        let json_file = conda_json_path();
        let json_file_noarch = conda_json_path_noarch();

        let repo_data = read_repodata(&json_file);
        let repo_data_noarch = read_repodata(&json_file_noarch);

        let available_packages = vec![repo_data, repo_data_noarch];

        let specs = vec![MatchSpec::from_str("python=3.9", &ChannelConfig::default()).unwrap()];

        let problem = SolverProblem {
            available_packages: available_packages
                .iter()
                .map(|records| LibsolvRepoData::from_records(&records)),
            specs,
            locked_packages: Default::default(),
            pinned_packages: Default::default(),
            virtual_packages: Default::default(),
        };

        let mut pkgs = LibsolvBackend
            .solve(problem)
            .unwrap()
            .into_iter()
            .map(|pkg| {
                format!(
                    "{} {} {}",
                    pkg.package_record.name, pkg.package_record.version, pkg.package_record.build
                )
            })
            .collect::<Vec<_>>();

        // The order of packages is nondeterministic, so we sort them to ensure we can compare them
        // to a previous run
        pkgs.sort();

        insta::assert_yaml_snapshot!(pkgs);
    }

    #[test]
    fn test_solve_dummy_repo_install_non_existent() {
        let result = solve(
            dummy_channel_json_path(),
            Vec::new(),
            Vec::new(),
            &["asdfasdf", "foo<4"],
            false,
        );

        assert!(result.is_err());

        let err = result.err().unwrap();
        assert!(matches!(err, SolveError::Unsolvable));
    }

    #[test]
    #[cfg(not(target_os = "nto"))]
    fn test_solve_with_cached_solv_file_install_new() -> anyhow::Result<()> {
        let pkgs = solve(
            dummy_channel_json_path(),
            Vec::new(),
            Vec::new(),
            &["foo<4"],
            true,
        )?;

        assert_eq!(1, pkgs.len());
        let info = &pkgs[0];

        assert_eq!("foo-3.0.2-py36h1af98f8_1.conda", info.file_name);
        assert_eq!(
            "https://conda.anaconda.org/conda-forge/linux-64/foo-3.0.2-py36h1af98f8_1.conda",
            info.url.to_string()
        );
        assert_eq!("https://conda.anaconda.org/conda-forge/", info.channel);
        assert_eq!("foo", info.package_record.name);
        assert_eq!("linux-64", info.package_record.subdir);
        assert_eq!("3.0.2", info.package_record.version.to_string());
        assert_eq!("py36h1af98f8_1", info.package_record.build);
        assert_eq!(1, info.package_record.build_number);
        assert_eq!(
            "67a63bec3fd3205170eaad532d487595b8aaceb9814d13c6858d7bac3ef24cd4",
            info.package_record.sha256.as_ref().unwrap()
        );
        assert_eq!(
            "fb731d9290f0bcbf3a054665f33ec94f",
            info.package_record.md5.as_ref().unwrap()
        );

        Ok(())
    }

    #[test]
    fn test_solve_dummy_repo_install_new() -> anyhow::Result<()> {
        let pkgs = solve(
            dummy_channel_json_path(),
            Vec::new(),
            Vec::new(),
            &["foo<4"],
            false,
        )?;

        assert_eq!(1, pkgs.len());
        let info = &pkgs[0];

        assert_eq!("foo-3.0.2-py36h1af98f8_1.conda", info.file_name);
        assert_eq!(
            "https://conda.anaconda.org/conda-forge/linux-64/foo-3.0.2-py36h1af98f8_1.conda",
            info.url.to_string()
        );
        assert_eq!("https://conda.anaconda.org/conda-forge/", info.channel);
        assert_eq!("foo", info.package_record.name);
        assert_eq!("linux-64", info.package_record.subdir);
        assert_eq!("3.0.2", info.package_record.version.to_string());
        assert_eq!("py36h1af98f8_1", info.package_record.build);
        assert_eq!(1, info.package_record.build_number);
        assert_eq!(
            "67a63bec3fd3205170eaad532d487595b8aaceb9814d13c6858d7bac3ef24cd4",
            info.package_record.sha256.as_ref().unwrap()
        );
        assert_eq!(
            "fb731d9290f0bcbf3a054665f33ec94f",
            info.package_record.md5.as_ref().unwrap()
        );

        Ok(())
    }

    #[test]
    fn test_solve_dummy_repo_prefers_conda_package() -> anyhow::Result<()> {
        // There following package is provided as .tar.bz and as .conda in repodata.json
        let match_spec = "foo=3.0.2=py36h1af98f8_1";

        let operations = solve(
            dummy_channel_json_path(),
            Vec::new(),
            Vec::new(),
            &[match_spec],
            false,
        )?;

        // The .conda entry is selected for installing
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].file_name, "foo-3.0.2-py36h1af98f8_1.conda");

        Ok(())
    }

    #[test]
    fn test_solve_dummy_repo_install_noop() -> anyhow::Result<()> {
        let already_installed = vec![installed_package(
            "conda-forge",
            "linux-64",
            "foo",
            "3.0.2",
            "py36h1af98f8_1",
            1,
        )];

        let pkgs = solve(
            dummy_channel_json_path(),
            already_installed,
            Vec::new(),
            &["foo<4"],
            false,
        )?;

        assert_eq!(1, pkgs.len());

        // Install
        let info = &pkgs[0];
        assert_eq!("foo", &info.package_record.name);
        assert_eq!("3.0.2", &info.package_record.version.to_string());

        Ok(())
    }

    #[test]
    fn test_solve_dummy_repo_upgrade() -> anyhow::Result<()> {
        let already_installed = vec![installed_package(
            "conda-forge",
            "linux-64",
            "foo",
            "3.0.2",
            "py36h1af98f8_1",
            1,
        )];

        let pkgs = solve(
            dummy_channel_json_path(),
            already_installed,
            Vec::new(),
            &["foo>=4"],
            false,
        )?;

        // Install
        let info = &pkgs[0];
        assert_eq!("foo", &info.package_record.name);
        assert_eq!("4.0.2", &info.package_record.version.to_string());

        Ok(())
    }

    #[test]
    fn test_solve_dummy_repo_downgrade() -> anyhow::Result<()> {
        let already_installed = vec![installed_package(
            "conda-forge",
            "linux-64",
            "foo",
            "4.0.2",
            "py36h1af98f8_1",
            1,
        )];

        let pkgs = solve(
            dummy_channel_json_path(),
            already_installed,
            Vec::new(),
            &["foo<4"],
            false,
        )?;

        assert_eq!(pkgs.len(), 1);

        // Uninstall
        let info = &pkgs[0];
        assert_eq!("foo", &info.package_record.name);
        assert_eq!("3.0.2", &info.package_record.version.to_string());

        Ok(())
    }

    #[test]
    fn test_solve_dummy_repo_remove() -> anyhow::Result<()> {
        let already_installed = vec![installed_package(
            "conda-forge",
            "linux-64",
            "foo",
            "3.0.2",
            "py36h1af98f8_1",
            1,
        )];

        let pkgs = solve(
            dummy_channel_json_path(),
            already_installed,
            Vec::new(),
            &[],
            false,
        )?;

        // Should be no packages!
        assert_eq!(0, pkgs.len());

        Ok(())
    }

    #[test]
    fn test_solve_dummy_repo_with_virtual_package() -> anyhow::Result<()> {
        let pkgs = solve(
            dummy_channel_json_path(),
            Vec::new(),
            vec![GenericVirtualPackage {
                name: "__unix".to_string(),
                version: Version::from_str("0").unwrap(),
                build_string: "0".to_string(),
            }],
            &["bar"],
            false,
        )?;

        assert_eq!(pkgs.len(), 1);

        let info = &pkgs[0];
        assert_eq!("bar", &info.package_record.name);
        assert_eq!("1.2.3", &info.package_record.version.to_string());

        Ok(())
    }

    #[test]
    fn test_solve_dummy_repo_missing_virtual_package() {
        let result = solve(
            dummy_channel_json_path(),
            Vec::new(),
            Vec::new(),
            &["bar"],
            false,
        );

        assert!(matches!(result.err(), Some(SolveError::Unsolvable)));
    }

    #[cfg(test)]
    fn solve(
        repo_path: String,
        installed_packages: Vec<RepoDataRecord>,
        virtual_packages: Vec<GenericVirtualPackage>,
        match_specs: &[&str],
        with_solv_file: bool,
    ) -> Result<Vec<RepoDataRecord>, SolveError> {
        let channel = Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap();

        let repo_data = read_repodata(&repo_path);

        #[cfg(not(target_os = "nto"))]
        let cached_repo_data = super::cache_libsolv_repodata(
            channel.platform_url(Platform::Linux64).to_string(),
            &repo_data,
        );

        let libsolv_repodata = if with_solv_file {
            #[cfg(target_os = "nto")]
            panic!("solv files are unsupported for this platform");

            #[cfg(not(target_os = "nto"))]
            LibsolvRepoData {
                records: repo_data.as_slice(),
                solv_file: Some(&cached_repo_data),
            }
        } else {
            LibsolvRepoData::from_records(&repo_data)
        };

        let channel_config = ChannelConfig::default();
        let specs = match_specs
            .into_iter()
            .map(|m| MatchSpec::from_str(m, &channel_config).unwrap())
            .collect();

        let problem = SolverProblem {
            locked_packages: installed_packages,
            virtual_packages,
            available_packages: vec![libsolv_repodata].into_iter(),
            specs,
            pinned_packages: Vec::new(),
        };

        let pkgs = LibsolvBackend.solve(problem)?;

        for pkg in pkgs.iter() {
            println!(
                "{} {} {}",
                pkg.package_record.name, pkg.package_record.version, pkg.package_record.build
            )
        }

        if pkgs.len() == 0 {
            println!("No packages in the environment!");
        }

        Ok(pkgs)
    }
}
