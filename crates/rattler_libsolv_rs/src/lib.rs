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
mod conda_util;
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
use rattler_conda_types::PackageRecord;
pub use solvable::{PackageSolvable, SolvableMetadata};
pub use solve_jobs::SolveJobs;
pub use solver::Solver;
use std::fmt::{Debug, Display};
use std::hash::Hash;
pub use transaction::Transaction;

use rattler_conda_types::MatchSpec;

/// Trait describing sets of versions.
pub trait VersionSet: Debug + Display + Clone + Eq + Hash {
    /// Version type associated with the sets manipulated.
    type V;

    /// Evaluate membership of a version in this set.
    fn contains(&self, v: &Self::V) -> bool;
}

impl VersionSet for MatchSpec {
    type V = PackageRecord;

    fn contains(&self, v: &Self::V) -> bool {
        self.matches(v)
    }
}
