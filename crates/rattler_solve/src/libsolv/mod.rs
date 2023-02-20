use std::collections::HashMap;
use std::ffi::CString;

mod ffi;
mod keys;
mod pool;
mod queue;
mod repo;
mod solvable;
mod solver;
mod transaction;

use pool::{Intern, Pool, Verbosity};
use queue::Queue;

use crate::libsolv::ffi::{
    SOLVER_ERASE, SOLVER_FLAG_ALLOW_DOWNGRADE, SOLVER_FLAG_ALLOW_UNINSTALL, SOLVER_INSTALL,
    SOLVER_SOLVABLE_PROVIDES, SOLVER_UPDATE,
};
use crate::{PackageOperation, RequestedAction, SolveError, SolverProblem};

/// Convenience method that converts a string reference to a CString
fn c_string<T: AsRef<str>>(str: T) -> CString {
    CString::new(str.as_ref()).expect("should be convertable from string")
}

fn request_to_solvable_flags(action: RequestedAction) -> u32 {
    match action {
        RequestedAction::Install => SOLVER_INSTALL,
        RequestedAction::Remove => SOLVER_ERASE,
        RequestedAction::Update => SOLVER_UPDATE,
    }
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
    for (channel, repodata) in problem.channels.iter() {
        let repo = pool.create_repo(channel);
        repo.add_repodata(repodata)
            .map_err(SolveError::ErrorAddingRepodata)?;
        channel_mapping.insert(repo.id(), channel.clone());

        // We dont want to drop the Repo, its stored in the pool anyway, so just forget it.
        std::mem::forget(repo);
    }

    let repo = pool.create_repo("installed");
    repo.add_installed(&problem.installed_packages)
        .map_err(SolveError::ErrorAddingInstalledPackages)?;
    pool.set_installed(&repo);

    // Create datastructures for solving
    pool.create_whatprovides();

    // Add matchspec to the queue
    let mut queue = Queue::default();
    for (spec, request) in problem.specs {
        let id = spec.intern(&pool);
        queue.push_id_with_flags(
            id,
            (request_to_solvable_flags(request) | SOLVER_SOLVABLE_PROVIDES) as i32,
        );
    }

    // Construct a solver and solve the problems in the queue
    let mut solver = pool.create_solver();
    solver.set_flag(SOLVER_FLAG_ALLOW_UNINSTALL, true);
    solver.set_flag(SOLVER_FLAG_ALLOW_DOWNGRADE, true);
    if solver.solve(&mut queue).is_err() {
        return Err(SolveError::Unsolvable);
    }

    // Construct a transaction from the solver
    let mut transaction = solver.create_transaction();
    let operations = transaction
        .get_solvable_operations(&channel_mapping)
        .map_err(|_unsupported_operation_ids| SolveError::UnsupportedOperations)?;

    Ok(operations)
}

#[cfg(test)]
mod test {
    use super::pool::Pool;
    use crate::package_operation::PackageOperation;
    use crate::package_operation::PackageOperationKind;
    use crate::{InstalledPackage, RequestedAction, SolveError, SolverProblem};
    use rattler_conda_types::{ChannelConfig, MatchSpec, RepoData};

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

    fn read_repodata(path: &str) -> RepoData {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
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

        // TODO: it looks like `libsolv` supports adding the repo_data directly from its json file,
        // maybe we should use that instead
        let repo_data = read_repodata(&json_file);
        let repo_data_noarch = read_repodata(&json_file_noarch);

        let channels = vec![
            ("conda-forge".to_string(), &repo_data),
            ("conda-forge".to_string(), &repo_data_noarch),
        ];

        let specs = vec![(
            MatchSpec::from_str("python=3.9", &ChannelConfig::default()).unwrap(),
            RequestedAction::Install,
        )];

        let problem = SolverProblem {
            channels,
            specs,
            installed_packages: Vec::new(),
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
        let repo_data = read_repodata(&dummy_channel_json_path());
        let problem = SolverProblem {
            channels: vec![("conda-forge".to_owned(), &repo_data)],
            specs: vec![
                (
                    MatchSpec::from_str("asdfasdf", &ChannelConfig::default()).unwrap(),
                    RequestedAction::Install,
                ),
                (
                    MatchSpec::from_str("foo", &ChannelConfig::default()).unwrap(),
                    RequestedAction::Install,
                ),
            ],
            installed_packages: Vec::new(),
        };

        let solved = problem.solve();
        assert!(solved.is_err());

        let err = solved.err().unwrap();
        assert!(matches!(err, SolveError::Unsolvable));
    }

    #[test]
    fn test_solve_dummy_repo_install_noop() {
        let already_installed = vec![InstalledPackage {
            name: "foo".to_string(),
            version: "3.0.2".to_string(),
            build_string: Some("py36h1af98f8_1".to_string()),
            build_number: Some(1),
        }];

        let operations = solve_with_already_installed(
            dummy_channel_json_path(),
            already_installed,
            &["foo<4"],
            RequestedAction::Install,
        );

        assert_eq!(0, operations.len());
    }

    #[test]
    fn test_solve_dummy_repo_upgrade() {
        let already_installed = vec![InstalledPackage {
            name: "foo".to_string(),
            version: "3.0.2".to_string(),
            build_string: Some("py36h1af98f8_1".to_string()),
            build_number: Some(1),
        }];

        let operations = solve_with_already_installed(
            dummy_channel_json_path(),
            already_installed,
            &["foo>=4"],
            RequestedAction::Update,
        );

        assert_eq!(2, operations.len());

        // Uninstall
        assert!(matches!(operations[0].kind, PackageOperationKind::Remove));
        let info = &operations[0].package;
        assert_eq!("foo", &info.name);
        assert_eq!("3.0.2", &info.version);

        // Install
        assert!(matches!(operations[1].kind, PackageOperationKind::Install));
        let info = &operations[1].package;
        assert_eq!("foo", &info.name);
        assert_eq!("4.0.2", &info.version);
    }

    #[test]
    fn test_solve_dummy_repo_downgrade() {
        let already_installed = vec![InstalledPackage {
            name: "foo".to_string(),
            version: "4.0.2".to_string(),
            build_string: Some("py36h1af98f8_1".to_string()),
            build_number: Some(1),
        }];

        let operations = solve_with_already_installed(
            dummy_channel_json_path(),
            already_installed,
            &["foo<4"],
            RequestedAction::Update,
        );

        assert_eq!(2, operations.len());

        // Uninstall
        assert!(matches!(operations[0].kind, PackageOperationKind::Remove));
        let info = &operations[0].package;
        assert_eq!("foo", &info.name);
        assert_eq!("4.0.2", &info.version);

        // Install
        assert!(matches!(operations[1].kind, PackageOperationKind::Install));
        let info = &operations[1].package;
        assert_eq!("foo", &info.name);
        assert_eq!("3.0.2", &info.version);
    }

    #[test]
    fn test_solve_dummy_repo_remove() {
        let already_installed = vec![InstalledPackage {
            name: "foo".to_string(),
            version: "3.0.2".to_string(),
            build_string: Some("py36h1af98f8_1".to_string()),
            build_number: Some(1),
        }];

        let operations = solve_with_already_installed(
            dummy_channel_json_path(),
            already_installed,
            &["foo"],
            RequestedAction::Remove,
        );

        assert_eq!(1, operations.len());

        // Uninstall
        assert!(matches!(operations[0].kind, PackageOperationKind::Remove));
        let info = &operations[0].package;
        assert_eq!("foo", &info.name);
        assert_eq!("3.0.2", &info.version);
    }

    #[cfg(test)]
    fn solve_with_already_installed(
        repo_path: String,
        installed_packages: Vec<InstalledPackage>,
        match_specs: &[&str],
        match_spec_action: RequestedAction,
    ) -> Vec<PackageOperation> {
        let repo_data =
            serde_json::from_str::<RepoData>(&std::fs::read_to_string(repo_path).unwrap()).unwrap();
        let channels = vec![("conda-forge".to_owned(), &repo_data)];
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
            channels,
            specs,
        };

        let solvable_operations = problem.solve().unwrap();

        for operation in solvable_operations.iter() {
            println!("{:?} - {:?}", operation.kind, operation.package);
        }

        if solvable_operations.len() == 0 {
            println!("No operations necessary!");
        }

        solvable_operations
    }
}
