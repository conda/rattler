use std::ffi::CString;

mod ffi;
mod pool;
mod queue;
mod repo;
mod solvable;
mod solver;
mod transaction;

pub use pool::{Intern, Pool, PoolRef, Verbosity};
pub use queue::Queue;
pub use repo::Repo;
pub use solver::Solver;
pub use transaction::{InstallOperation, Transaction, TransactionRef};

pub use ffi::{SOLVER_INSTALL, SOLVER_SOLVABLE_PROVIDES};

/// Convenience method that converts a string reference to a CString
fn c_string<T: AsRef<str>>(str: T) -> CString {
    CString::new(str.as_ref()).expect("should be convertable from string")
}

#[cfg(test)]
mod test {
    use crate::libsolv::ffi::{SOLVER_INSTALL, SOLVER_SOLVABLE_PROVIDES};
    use crate::libsolv::pool::{Intern, Pool};
    use crate::libsolv::queue::Queue;
    use rattler::{ChannelConfig, MatchSpec};

    use super::pool::Verbosity;

    fn conda_json_path() -> String {
        format!(
            "{}/{}",
            env!("CARGO_MANIFEST_DIR"),
            "resources/channels/conda-forge/linux-64/repodata.json"
        )
    }

    fn conda_json_path_noarch() -> String {
        format!(
            "{}/{}",
            env!("CARGO_MANIFEST_DIR"),
            "resources/channels/conda-forge/noarch/repodata.json"
        )
    }

    #[test]
    fn test_conda_read_repodata() {
        let json_file = conda_json_path();
        let mut pool = Pool::default();
        let mut repo = pool.create_repo("conda-forge");
        repo.add_conda_json(json_file)
            .expect("could not add repodata to Repo");
    }

    #[test]
    fn test_solve_python() {
        let json_file = conda_json_path();
        let json_file_noarch = conda_json_path_noarch();
        let mut pool = Pool::default();
        pool.set_debug_callback(|msg| eprintln!("{}", msg.trim()));
        pool.set_debug_level(Verbosity::Low);
        let mut repo = pool.create_repo("conda-forge");
        repo.add_repodata(
            &serde_json::from_str(&std::fs::read_to_string(json_file).expect("couldnt read"))
                .expect("couldnt parse"),
        )
        .expect("cannot add repodata");
        repo.add_repodata(
            &serde_json::from_str(&std::fs::read_to_string(json_file_noarch).unwrap()).unwrap(),
        )
        .unwrap();
        // repo.add_conda_json(json_file)
        //     .expect("could not add repodata to Repo");
        // repo.add_conda_json(json_file_noarch)
        //     .expect("could not add repodata (noarch) to Repo");
        // Create datastructures for solving
        pool.create_whatprovides();
        let channel_config = ChannelConfig::default();

        // Creat python as a matchspec
        let matchspec =
            MatchSpec::from_str("python=3.9", &channel_config).expect("can't create matchspec");
        // Add matchspec to the queue
        let mut queue = Queue::default();
        let id = matchspec.intern(&mut pool);
        queue.push_id_and_flags(id, (SOLVER_INSTALL | SOLVER_SOLVABLE_PROVIDES) as i32);
        // Solver
        let mut solver = pool.create_solver();
        // solve
        solver.solve(&mut queue).expect("unable to solve");

        let mut transaction = solver.create_transaction();
        let solvable_operations = transaction.get_solvable_operations();
        for operation in solvable_operations.iter() {
            println!(
                "{:?} - {:?}",
                operation.operation,
                operation.solvable.solvable_info()
            );
        }
    }
}
