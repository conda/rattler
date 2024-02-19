use std::{ffi::CStr, marker::PhantomData, ptr::NonNull};

use super::{
    ffi, flags::SolverFlag, pool::Pool, queue::Queue, solvable::SolvableId, solve_goal::SolveGoal,
    solve_problem::SolveProblem, transaction::Transaction,
};

/// Wrapper for libsolv solver, which is used to drive dependency resolution
///
/// The wrapper functions as an owned pointer, guaranteed to be non-null and freed
/// when the Solver is dropped
pub struct Solver<'pool>(NonNull<ffi::Solver>, PhantomData<&'pool Pool>);

impl<'pool> Drop for Solver<'pool> {
    fn drop(&mut self) {
        unsafe { ffi::solver_free(self.0.as_mut()) }
    }
}

impl<'pool> Solver<'pool> {
    /// Constructs a new Solver from the provided libsolv pointer. It is the responsibility of the
    /// caller to ensure the pointer is actually valid.
    pub(super) unsafe fn new(_pool: &Pool, ptr: NonNull<ffi::Solver>) -> Solver<'_> {
        Solver(ptr, PhantomData)
    }

    /// Returns a raw pointer to the wrapped `ffi::Solver`, to be used for calling ffi functions
    /// that require access to the pool (and for nothing else)
    fn raw_ptr(&self) -> *mut ffi::Solver {
        self.0.as_ptr()
    }

    /// Returns the amount of problems that are yet to be solved
    fn problem_count(&self) -> u32 {
        unsafe { ffi::solver_problem_count(self.raw_ptr()) }
    }

    /// Returns a user-friendly representation of a problem
    ///
    /// Safety: the caller must ensure the id is valid
    unsafe fn problem2str(&self, id: ffi::Id) -> &CStr {
        let problem = ffi::solver_problem2str(self.raw_ptr(), id);
        CStr::from_ptr(problem)
    }

    /// Creates a string for each 'problem' that the solver still has which it encountered while
    /// solving the matchspecs. Use this function to print the existing problems to string.
    fn solver_problems(&self) -> Vec<String> {
        let mut output = Vec::new();

        let count = self.problem_count();
        for i in 1..=count {
            // Safe because the id valid (between [1, count])
            let problem = unsafe { self.problem2str(i as ffi::Id) };

            output.push(
                problem
                    .to_str()
                    .expect("string is invalid UTF8")
                    .to_string(),
            );
        }
        output
    }

    pub fn all_solver_problems(&self) -> Vec<SolveProblem> {
        let mut problems = Vec::new();
        let mut problem_rules = Queue::<ffi::Id>::default();

        let count = self.problem_count();
        for i in 1..=count {
            unsafe {
                ffi::solver_findallproblemrules(
                    self.raw_ptr(),
                    i.try_into().unwrap(),
                    problem_rules.raw_ptr(),
                );
            };
            for r in problem_rules.id_iter() {
                if r != 0 {
                    let mut source_id = 0;
                    let mut target_id = 0;
                    let mut dep_id = 0;

                    let problem_type = unsafe {
                        ffi::solver_ruleinfo(
                            self.raw_ptr(),
                            r,
                            &mut source_id,
                            &mut target_id,
                            &mut dep_id,
                        )
                    };

                    let pool: *mut ffi::Pool = unsafe { (*self.0.as_ptr()).pool.cast() };

                    let nsolvables = unsafe { (*pool).nsolvables };

                    let target = if target_id < 0 || target_id >= nsolvables {
                        None
                    } else {
                        Some(SolvableId(target_id))
                    };

                    let source = if source_id < 0 || source_id >= nsolvables {
                        None
                    } else {
                        Some(SolvableId(target_id))
                    };

                    let dep = if dep_id == 0 {
                        None
                    } else {
                        let dep = unsafe { ffi::pool_dep2str(pool, dep_id) };
                        let dep = unsafe { CStr::from_ptr(dep) };
                        let dep = dep.to_str().expect("Invalid UTF8 value").to_string();
                        Some(dep)
                    };

                    problems.push(SolveProblem::from_raw(problem_type, dep, source, target));
                }
            }
        }
        problems
    }

    /// Sets a solver flag
    pub fn set_flag(&self, flag: SolverFlag, value: bool) {
        unsafe { ffi::solver_set_flag(self.raw_ptr(), flag.inner(), i32::from(value)) };
    }

    /// Solves all the problems in the `queue` and returns a transaction from the found solution.
    /// Returns an error if problems remain unsolved.
    pub fn solve(&mut self, queue: &mut SolveGoal) -> Result<Transaction<'_>, Vec<String>> {
        let result = unsafe {
            // Run the solve method
            ffi::solver_solve(self.raw_ptr(), queue.raw_ptr());
            // If there are no problems left then the solver is done
            ffi::solver_problem_count(self.raw_ptr()) == 0
        };
        if result {
            let transaction =
                NonNull::new(unsafe { ffi::solver_create_transaction(self.raw_ptr()) })
                    .expect("solver_create_transaction returned a nullptr");

            // Safe because we know the `transaction` ptr is valid
            Ok(unsafe { Transaction::new(self, transaction) })
        } else {
            Err(self.solver_problems())
        }
    }
}
