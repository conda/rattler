use std::ffi::CStr;
use std::fmt::Write;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

use anyhow::anyhow;

use crate::libsolv::ffi;
use crate::libsolv::pool::PoolRef;
use crate::libsolv::queue::Queue;
use crate::libsolv::transaction::Transaction;

/// Wraps a pointer to an `ffi::Solver` which is freed when the instance is dropped.
struct SolverOwnedPtr(NonNull<ffi::Solver>);

impl Drop for SolverOwnedPtr {
    fn drop(&mut self) {
        unsafe { ffi::solver_free(self.0.as_mut()) }
    }
}

/// Representation of a repo containing package data in libsolv. This corresponds to a repo_data
/// json. Lifetime of this object is coupled to the Pool on creation
pub struct Solver<'pool>(SolverOwnedPtr, PhantomData<&'pool ffi::Solver>);

/// A `SolverRef` is a wrapper around an `ffi::Solver` that provides a safe abstraction
/// over its functionality.
///
/// A `SolverRef` can not be constructed by itself but is instead returned by dereferencing a
/// [`Solver`].
#[repr(transparent)]
pub struct SolverRef(ffi::Solver);

impl Deref for Solver<'_> {
    type Target = SolverRef;

    fn deref(&self) -> &Self::Target {
        unsafe { self.0 .0.cast().as_ref() }
    }
}

impl DerefMut for Solver<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.0 .0.cast().as_mut() }
    }
}

impl SolverRef {
    /// Returns a pointer to the wrapped `ffi::Solver`
    fn as_ptr(&self) -> NonNull<ffi::Solver> {
        // Safe because a `SolverRef` is a transparent wrapper around `ffi::Solver`
        unsafe { NonNull::new_unchecked(self as *const Self as *mut Self).cast() }
    }

    /// Returns a reference to the wrapped `ffi::Solver`.
    fn as_ref(&self) -> &ffi::Solver {
        // Safe because a `SolverRef` is a transparent wrapper around `ffi::Solver`
        unsafe { std::mem::transmute(self) }
    }

    /// Returns the pool that created this instance
    pub fn pool(&self) -> &PoolRef {
        // Safe because a `PoolRef` is a wrapper around `ffi::Pool`
        unsafe { &*(self.as_ref().pool as *const PoolRef) }
    }

    /// Creates a string of 'problems' that the solver still has which it encountered while solving
    /// the matchspecs use this function to print the existing problems to string
    fn solver_problems(&self) -> String {
        let mut problem_queue = Queue::default();
        let count = unsafe { ffi::solver_problem_count(self.as_ptr().as_ptr()) as u32 };
        let mut output = String::default();
        for i in 1..=count {
            problem_queue.push_id(i as ffi::Id);
            let problem = unsafe {
                let problem = ffi::solver_problem2str(self.as_ptr().as_ptr(), i as ffi::Id);
                CStr::from_ptr(problem)
                    .to_str()
                    .expect("could not parse string")
            };
            writeln!(&mut output, " - {}", problem).expect("could not write into string");
        }
        output
    }

    /// Run the libsolv solver that solves the problems, which are probably matchspecs in the pool
    pub fn solve<T>(&mut self, queue: &mut Queue<T>) -> anyhow::Result<()> {
        let result = unsafe {
            // Run the solve method
            ffi::solver_solve(self.as_ptr().as_mut(), queue.as_inner_mut());
            // If there are no problems left then the solver is done
            ffi::solver_problem_count(self.as_ptr().as_mut()) == 0
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

    /// Create a transaction from the solver
    pub fn create_transaction(&mut self) -> Transaction {
        let transaction =
            NonNull::new(unsafe { ffi::solver_create_transaction(self.as_ptr().as_mut()) })
                .expect("solver_create_transaction returned a nullptr");
        Transaction::new(transaction)
    }
}

impl Solver<'_> {
    /// Constructs a new instance
    pub(super) fn new(ptr: NonNull<ffi::Solver>) -> Self {
        Solver(SolverOwnedPtr(ptr), PhantomData::default())
    }
}
