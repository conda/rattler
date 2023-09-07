//! Implements a SAT solver for dependency resolution based on the CDCL algorithm (conflict-driven
//! clause learning)
//!
//! The CDCL algorithm is masterly explained in [An Extensible
//! SAT-solver](http://minisat.se/downloads/MiniSat.pdf). Regarding the data structures used, we
//! mostly follow the approach taken by [libsolv](https://github.com/openSUSE/libsolv). The code of
//! libsolv is, however, very low level C, so if you are looking for an introduction to CDCL, you
//! are encouraged to look at the paper instead or to keep reading through this codebase and its
//! comments.

#![deny(missing_docs)]

mod arena;
mod id;
mod mapping;
mod pool;
pub mod problem;
mod solvable;
mod solve_jobs;
mod solver;
mod transaction;

pub use id::{NameId, RepoId, SolvableId, VersionSetId};
pub use pool::Pool;
pub use solvable::{PackageSolvable, SolvableMetadata};
pub use solve_jobs::SolveJobs;
pub use solver::Solver;
use std::cell::OnceCell;
use std::fmt::{Debug, Display};
use std::hash::Hash;
pub use transaction::Transaction;

pub use mapping::Mapping;

/// Version is a name and a version specification.
pub trait VersionTrait: Display {
    /// The name of the package associated with this record.
    /// TODO: Can we move this to the Pool?
    type Name: Display + Sized + Hash + Eq + Clone;

    /// The version associated with this record.
    type Version: Display + Ord + Clone;

    /// Returns the version associated with this record
    // TODO: We could maybe get rid of this, but would need to know what is generic to display and replace sorting in `problem.rs`
    fn version(&self) -> Self::Version;
}

/// Trait describing sets of versions.
pub trait VersionSet: Debug + Display + Clone + Eq + Hash {
    /// Version type associated with the sets manipulated.
    type V: VersionTrait;

    /// Evaluate membership of a version in this set.
    fn contains(&self, v: &Self::V) -> bool;
}

/// Bla
pub trait DependencyProvider<VS: VersionSet> {
    /// Sort the specified solvables based on which solvable to try first.
    fn sort_candidates(
        &mut self,
        pool: &Pool<VS>,
        solvables: &mut [SolvableId],
        match_spec_to_candidates: &Mapping<VersionSetId, OnceCell<Vec<SolvableId>>>,
    );
}
