use std::ffi::CString;

mod ffi;
mod pool;
mod queue;
mod repo;
mod solver;

/// Convenience method to convert from a string reference to a CString
fn c_string<T: AsRef<str>>(str: T) -> CString {
    CString::new(str.as_ref()).expect("could never be null because of trait-bound")
}

#[cfg(test)]
mod test {
    use crate::libsolv::ffi::{SOLVER_INSTALL, SOLVER_SOLVABLE_PROVIDES};
    use crate::libsolv::pool::{Intern, Pool};
    use crate::libsolv::queue::Queue;
    use rattler::{ChannelConfig, MatchSpec};

    #[test]
    fn test_conda_read_repodata() {
        let json_file = format!(
            "{}/{}",
            env!("CARGO_MANIFEST_DIR"),
            "resources/conda_forge_noarch_repodata.json"
        );
        let mut pool = Pool::default();
        let mut repo = pool.create_repo("conda-forge");
        repo.add_conda_json(json_file)
            .expect("could not add repodata to Repo");
    }

    #[test]
    fn test_solve_python() {
        let json_file = format!(
            "{}/{}",
            env!("CARGO_MANIFEST_DIR"),
            "resources/conda_forge_noarch_repodata.json"
        );
        let mut pool = Pool::default();
        let mut repo = pool.create_repo("conda-forge");
        repo.add_conda_json(json_file)
            .expect("could not add repodata to Repo");
        // Create datastructures for solving
        pool.create_whatprovides();
        let channel_config = ChannelConfig::default();

        // Creat python as a matchspec
        let matchspec =
            MatchSpec::from_str("python", &channel_config).expect("can't create matchspec");
        // Add matchspec to the queue
        let mut queue = Queue::default();
        let id = matchspec.intern(&mut pool);
        queue.push2(id, (SOLVER_INSTALL | SOLVER_SOLVABLE_PROVIDES) as i32);
        // Solver
    }
}
