use flags::{SolvableFlags, SolverFlag};
use std::collections::HashMap;
use std::ffi::CString;

mod custom_keys;
mod ffi;
mod flags;
mod keys;
mod pool;
mod queue;
mod repo;
mod solvable;
mod solver;
mod transaction;

use pool::{Intern, Pool, Verbosity};
use queue::Queue;

use crate::{PackageOperation, SolveError, SolverProblem};

/// Convenience method that converts a string reference to a CString
fn c_string<T: AsRef<str>>(str: T) -> CString {
    CString::new(str.as_ref()).expect("should be convertable from string")
}

pub fn solve(problem: SolverProblem) -> Result<Vec<PackageOperation>, SolveError> {
    // Construct a default libsolv pool
    let pool = Pool::default();

    // Setup proper logging for the pool
    pool.set_debug_callback(|msg, flags| {
        tracing::event!(tracing::Level::DEBUG, flags, "{}", msg);
    });
    pool.set_debug_level(Verbosity::Low);

    // Create repos for all channels
    let mut channel_mapping = HashMap::new();
    for repodata_records in &problem.available_packages {
        if repodata_records.is_empty() {
            continue;
        }

        let channel_name = &repodata_records[0].channel;
        let repo = pool.create_repo(channel_name);
        repo.add_repodata_records(repodata_records)
            .map_err(SolveError::ErrorAddingRepodata)?;
        channel_mapping.insert(repo.id(), channel_name.clone());

        // We dont want to drop the Repo, its stored in the pool anyway, so just forget it.
        std::mem::forget(repo);
    }

    let repo = pool.create_repo("installed");
    repo.add_installed(&problem.installed_packages)
        .map_err(SolveError::ErrorAddingInstalledPackages)?;
    repo.add_virtual_packages(&problem.virtual_packages)
        .map_err(SolveError::ErrorAddingInstalledPackages)?;
    pool.set_installed(&repo);

    // Create datastructures for solving
    pool.create_whatprovides();

    // Add matchspec to the queue
    let mut queue = Queue::default();
    for (spec, request) in problem.specs {
        let id = spec.intern(&pool);
        queue.push_id_with_flags(id, SolvableFlags::from(request));
    }

    // Construct a solver and solve the problems in the queue
    let mut solver = pool.create_solver();
    solver.set_flag(SolverFlag::allow_uninstall(), true);
    solver.set_flag(SolverFlag::allow_downgrade(), true);
    if solver.solve(&mut queue).is_err() {
        return Err(SolveError::Unsolvable);
    }

    // Construct a transaction from the solver
    let mut transaction = solver.create_transaction();
    let operations = transaction
        .get_package_operations(&channel_mapping)
        .map_err(|unsupported_operation_ids| {
            SolveError::UnsupportedOperations(
                unsupported_operation_ids
                    .into_iter()
                    .map(|id| format!("libsolv operation {id}"))
                    .collect(),
            )
        })?;

    Ok(operations)
}

#[cfg(test)]
mod test {
    use super::pool::Pool;
    use crate::package_operation::PackageOperation;
    use crate::package_operation::PackageOperationKind;
    use crate::{RequestedAction, SolveError, SolverProblem};
    use rattler_conda_types::prefix_record::PrefixPaths;
    use rattler_conda_types::{
        Channel, ChannelConfig, MatchSpec, NoArchType, PackageRecord, PrefixRecord, RepoData,
        RepoDataRecord, Version,
    };
    use rattler_virtual_packages::GenericVirtualPackage;
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
    ) -> PrefixRecord {
        PrefixRecord {
            package_tarball_full_path: None,
            extracted_package_dir: None,
            files: Vec::new(),
            paths_data: PrefixPaths::default(),
            link: None,
            requested_spec: None,
            repodata_record: RepoDataRecord {
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
            },
        }
    }

    #[test]
    fn test_conda_read_repodata() {
        let json_file = conda_json_path();
        let pool = Pool::default();
        let repo = pool.create_repo("conda-forge");
        repo.add_conda_json(json_file)
            .expect("could not add repodata to Repo");
    }

    #[test]
    fn test_solve_python() {
        let json_file = conda_json_path();
        let json_file_noarch = conda_json_path_noarch();

        let repo_data = read_repodata(&json_file);
        let repo_data_noarch = read_repodata(&json_file_noarch);

        let available_packages = vec![repo_data, repo_data_noarch];

        let specs = vec![(
            MatchSpec::from_str("python=3.9", &ChannelConfig::default()).unwrap(),
            RequestedAction::Install,
        )];

        let problem = SolverProblem {
            available_packages,
            specs,
            installed_packages: Vec::new(),
            virtual_packages: Vec::new(),
        };
        let operations = problem.solve().unwrap();
        for operation in operations.iter() {
            println!("{:?} - {:?}", operation.kind, operation.package);
        }

        assert!(
            operations.len() > 0,
            "no operations resulted from installing python!"
        );
    }

    #[test]
    fn test_solve_dummy_repo_install_non_existent() {
        let result = solve(
            dummy_channel_json_path(),
            Vec::new(),
            Vec::new(),
            &["asdfasdf", "foo<4"],
            RequestedAction::Install,
        );

        assert!(result.is_err());

        let err = result.err().unwrap();
        assert!(matches!(err, SolveError::Unsolvable));
    }

    #[test]
    fn test_solve_dummy_repo_install_new() -> anyhow::Result<()> {
        let operations = solve(
            dummy_channel_json_path(),
            Vec::new(),
            Vec::new(),
            &["foo<4"],
            RequestedAction::Install,
        )?;

        assert_eq!(1, operations.len());
        assert!(matches!(operations[0].kind, PackageOperationKind::Install));
        let info = &operations[0].package;

        assert_eq!("foo-3.0.2.tar.bz2", info.file_name);
        assert_eq!(
            "https://conda.anaconda.org/conda-forge/linux-64/foo-3.0.2.tar.bz2",
            info.url.to_string()
        );
        assert_eq!("https://conda.anaconda.org/conda-forge/", info.channel);
        assert_eq!("foo", info.package_record.name);
        assert_eq!("linux-64", info.package_record.subdir);
        assert_eq!("3.0.2", info.package_record.version.to_string());
        assert_eq!("py36h1af98f8_1", info.package_record.build);
        assert_eq!(1, info.package_record.build_number);
        assert_eq!(
            "1154fceeb5c4ee9bb97d245713ac21eb1910237c724d2b7103747215663273c2",
            info.package_record.sha256.as_ref().unwrap()
        );
        assert_eq!(
            "d65ab674acf3b7294ebacaec05fc5b54",
            info.package_record.md5.as_ref().unwrap()
        );

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

        let operations = solve(
            dummy_channel_json_path(),
            already_installed,
            Vec::new(),
            &["foo<4"],
            RequestedAction::Install,
        )?;

        assert_eq!(0, operations.len());

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

        let operations = solve(
            dummy_channel_json_path(),
            already_installed,
            Vec::new(),
            &["foo>=4"],
            RequestedAction::Update,
        )?;

        assert_eq!(2, operations.len());

        // Uninstall
        assert!(matches!(operations[0].kind, PackageOperationKind::Remove));
        let info = &operations[0].package;
        assert_eq!("foo", &info.package_record.name);
        assert_eq!("3.0.2", &info.package_record.version.to_string());

        // Install
        assert!(matches!(operations[1].kind, PackageOperationKind::Install));
        let info = &operations[1].package;
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

        let operations = solve(
            dummy_channel_json_path(),
            already_installed,
            Vec::new(),
            &["foo<4"],
            RequestedAction::Update,
        )?;

        assert_eq!(2, operations.len());

        // Uninstall
        assert!(matches!(operations[0].kind, PackageOperationKind::Remove));
        let info = &operations[0].package;
        assert_eq!("foo", &info.package_record.name);
        assert_eq!("4.0.2", &info.package_record.version.to_string());

        // Install
        assert!(matches!(operations[1].kind, PackageOperationKind::Install));
        let info = &operations[1].package;
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

        let operations = solve(
            dummy_channel_json_path(),
            already_installed,
            Vec::new(),
            &["foo"],
            RequestedAction::Remove,
        )?;

        assert_eq!(1, operations.len());

        // Uninstall
        assert!(matches!(operations[0].kind, PackageOperationKind::Remove));
        let info = &operations[0].package;
        assert_eq!("foo", &info.package_record.name);
        assert_eq!("3.0.2", &info.package_record.version.to_string());

        Ok(())
    }

    #[test]
    fn test_solve_dummy_repo_with_virtual_package() -> anyhow::Result<()> {
        let operations = solve(
            dummy_channel_json_path(),
            Vec::new(),
            vec![GenericVirtualPackage {
                name: "__unix".to_string(),
                version: Version::from_str("0").unwrap(),
                build_string: "0".to_string(),
            }],
            &["bar"],
            RequestedAction::Install,
        )?;

        assert_eq!(1, operations.len());
        assert_eq!(PackageOperationKind::Install, operations[0].kind);
        let info = &operations[0].package;
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
            RequestedAction::Install,
        );

        assert!(matches!(result.err(), Some(SolveError::Unsolvable)));
    }

    #[cfg(test)]
    fn solve(
        repo_path: String,
        installed_packages: Vec<PrefixRecord>,
        virtual_packages: Vec<GenericVirtualPackage>,
        match_specs: &[&str],
        match_spec_action: RequestedAction,
    ) -> Result<Vec<PackageOperation>, SolveError> {
        let repo_data = read_repodata(&repo_path);
        let available_packages = vec![repo_data];
        let channel_config = ChannelConfig::default();
        let specs = match_specs
            .into_iter()
            .map(|m| {
                (
                    MatchSpec::from_str(m, &channel_config).unwrap(),
                    match_spec_action,
                )
            })
            .collect();

        let problem = SolverProblem {
            installed_packages,
            virtual_packages,
            available_packages,
            specs,
        };

        let solvable_operations = problem.solve()?;

        for operation in solvable_operations.iter() {
            println!("{:?} - {:?}", operation.kind, operation.package);
        }

        if solvable_operations.len() == 0 {
            println!("No operations necessary!");
        }

        Ok(solvable_operations)
    }
}
