use crate::libsolv::ffi;
use crate::libsolv::queue::Queue;
use anyhow::anyhow;
use std::ffi::CStr;
use std::fmt::Write;
use std::marker::PhantomData;
use std::ptr::NonNull;

/// Representation of a repo containing package data in libsolv
/// This corresponds to a repo_data json
/// Lifetime of this object is coupled to the Pool on creation
pub struct Solver<'pool>(
    pub(super) SolverOwnedPtr,
    pub(super) PhantomData<&'pool ffi::Pool>,
);

/// Wrapper type so we do not use lifetime in the drop
pub(super) struct SolverOwnedPtr(NonNull<ffi::Solver>);

impl SolverOwnedPtr {
    pub fn new(solver: *mut ffi::Solver) -> SolverOwnedPtr {
        Self(NonNull::new(solver).expect("Could not create solver object"))
    }
}

impl Solver<'_> {
    /// Solver problems to string
    fn solver_problems(&self) -> String {
        let mut problem_queue = Queue::default();
        let count = unsafe { ffi::solver_problem_count(self.0 .0.as_ptr()) as u32 };
        let mut output = String::default();
        for i in 1..=count {
            problem_queue.push(i as ffi::Id);
            let problem = unsafe {
                let problem = ffi::solver_problem2str(self.0 .0.as_ptr(), i as ffi::Id);
                CStr::from_ptr(problem)
                    .to_str()
                    .expect("could not parse string")
            };
            write!(&mut output, " - {} \n", problem).expect("could not write into string");
        }
        output
    }

    /// Run the libsolv solver that solves the problems, which are probably matchspecs in the pool
    pub fn solve<T>(&mut self, queue: &mut Queue<T>) -> anyhow::Result<()> {
        let result = unsafe {
            // Run the solve method
            ffi::solver_solve(self.0 .0.as_mut(), queue.as_inner_mut());
            // If there are no problems left then the solver is done
            ffi::solver_problem_count(self.0 .0.as_mut()) == 0
        };
        if result {
            Ok(())
        } else {
            Err(anyhow!(
                "encountered problems while solving:\n {}",
                self.solver_problems()
            ))
        }
    }
}
