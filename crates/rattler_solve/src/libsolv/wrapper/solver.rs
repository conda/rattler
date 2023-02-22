use std::ffi::CStr;
use std::marker::PhantomData;
use std::ptr::NonNull;

use crate::libsolv::wrapper::pool::Pool;
use anyhow::anyhow;

use super::ffi;
use super::flags::SolverFlag;
use super::queue::Queue;
use super::transaction::Transaction;

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

impl Solver<'_> {
    /// Constructs a new Solver from the provided libsolv pointer. It is the responsibility of the
    /// caller to ensure the pointer is actually valid.
    pub(super) unsafe fn new(_pool: &Pool, ptr: NonNull<ffi::Solver>) -> Solver {
        Solver(ptr, PhantomData::default())
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

    /// Creates a string of 'problems' that the solver still has which it encountered while solving
    /// the matchspecs. Use this function to print the existing problems to string.
    fn solver_problems(&self) -> String {
        let mut output = String::default();

        let count = self.problem_count();
        for i in 1..=count {
            // Safe because the id valid (between [1, count])
            let problem = unsafe { self.problem2str(i as ffi::Id) };

            output.push_str(" - ");
            output.push_str(problem.to_str().expect("string is invalid UTF8"));
            output.push('\n');
        }
        output
    }

    /// Sets a solver flag
    pub fn set_flag(&self, flag: SolverFlag, value: bool) {
        unsafe { ffi::solver_set_flag(self.raw_ptr(), flag.inner(), i32::from(value)) };
    }

    /// Solves all the problems in the `queue`, or returns an error if problems remain.
    pub fn solve<T>(&mut self, queue: &mut Queue<T>) -> anyhow::Result<()> {
        let result = unsafe {
            // Run the solve method
            ffi::solver_solve(self.raw_ptr(), queue.raw_ptr());
            // If there are no problems left then the solver is done
            ffi::solver_problem_count(self.raw_ptr()) == 0
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

    /// Creates a transaction from the solutions found by the solver.
    pub fn create_transaction(&self) -> Transaction {
        let transaction = NonNull::new(unsafe { ffi::solver_create_transaction(self.raw_ptr()) })
            .expect("solver_create_transaction returned a nullptr");

        // Safe because we know the `transaction` ptr is valid
        unsafe { Transaction::new(self, transaction) }
    }
}
