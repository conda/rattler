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

pub use id::{NameId, SolvableId, VersionSetId};
use itertools::Itertools;
pub use pool::Pool;
pub use solvable::PackageSolvable;
pub use solve_jobs::SolveJobs;
pub use solver::Solver;
use std::cell::OnceCell;
use std::fmt::{Debug, Display};
use std::hash::Hash;
pub use transaction::Transaction;

pub use mapping::Mapping;

/// Blanket trait implementation for something that we consider a package name.
pub trait PackageName: Eq + Hash {}
impl<N: Eq + Hash> PackageName for N {}

/// Trait describing sets of versions.
pub trait VersionSet: Debug + Display + Clone + Eq + Hash {
    /// Version type associated with the sets manipulated.
    type V: Display + Ord;

    /// Evaluate membership of a version in this set.
    fn contains(&self, v: &Self::V) -> bool;
}
/// Describes how to sort tentative candidates, for a specific dependency provider. E.g conda
/// pypi, etc.
pub trait DependencyProvider<VS: VersionSet, N: PackageName = String> {
    /// Sort the specified solvables based on which solvable to try first.
    fn sort_candidates(
        &mut self,
        pool: &Pool<VS, N>,
        solvables: &mut [SolvableId],
        match_spec_to_candidates: &Mapping<VersionSetId, OnceCell<Vec<SolvableId>>>,
    );
}
/// Defines how merged candidates should be displayed.
pub trait SolvableDisplay<VS: VersionSet, Name: PackageName = String> {
    /// A method that is used to display multiple solvables in a user friendly way.
    /// For example the conda provider should only display the versions (not build strings etc.)
    /// and merges multiple solvables into one line.
    fn display_candidates(&self, pool: &Pool<VS, Name>, candidates: &[SolvableId]) -> String;
}

/// Display merged candidates on single line with `|` as separator.
pub struct DefaultSolvableDisplay;

impl<VS: VersionSet, Name: Hash + Eq> SolvableDisplay<VS, Name> for DefaultSolvableDisplay
where
    VS::V: Ord,
{
    fn display_candidates(
        &self,
        pool: &Pool<VS, Name>,
        merged_candidates: &[SolvableId],
    ) -> String {
        merged_candidates
            .iter()
            .map(|&id| &pool.resolve_solvable(id).inner)
            .sorted()
            .map(|s| s.to_string())
            .join(" | ")
    }
}
