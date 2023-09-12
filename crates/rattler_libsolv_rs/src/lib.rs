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
mod frozen_copy_map;
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
use std::fmt::{Debug, Display};
use std::hash::Hash;
pub use transaction::Transaction;

pub(crate) use frozen_copy_map::FrozenCopyMap;
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
pub trait DependencyProvider<VS: VersionSet, N: PackageName = String>: Sized {
    /// Returns the [`Pool`] that is used to allocate the Ids returned from this instance
    fn pool(&self) -> &Pool<VS, N>;

    /// Sort the specified solvables based on which solvable to try first.
    fn sort_candidates(&self, solvables: &mut [SolvableId], solver: &Solver<VS, N, Self>);

    /// Returns a list of solvables that should be considered when a package with the given name is
    /// requested.
    ///
    /// Returns `None` if no such package exist.
    fn get_candidates(&self, name: NameId) -> Option<Candidates>;

    /// Returns the dependencies for the specified solvable.
    fn get_dependencies(&self, solvable: SolvableId) -> Dependencies;
}

/// A list of candidate solvables for a specific package. This is returned from
/// [`DependencyProvider::get_candidates`].
#[derive(Default, Clone, Debug)]
pub struct Candidates {
    /// A list of all solvables for the package.
    pub candidates: Vec<SolvableId>,

    /// Optionally the id of the solvable that is favored over other solvables. The solver will
    /// first attempt to solve for the specified solvable but will fall back to other candidates if
    /// no solution could be found otherwise.
    ///
    /// The same behavior can be achieved by sorting this candidate to the top using the
    /// [`DependencyProvider::sort_candidates`] function but using this method providers better
    /// error messages to the user.
    pub favored: Option<SolvableId>,

    /// If specified this is the Id of the only solvable that can be selected. Although it would
    /// also be possible to simply return a single candidate using this field provides better error
    /// messages to the user.
    pub locked: Option<SolvableId>,
}

/// Holds information about the dependencies of a package.
#[derive(Default, Clone, Debug)]
pub struct Dependencies {
    /// Defines which packages should be installed alongside the depending package and the
    /// constraints applied to the package.
    pub requirements: Vec<VersionSetId>,

    /// Defines additional constraints on packages that may or may not be part of the solution.
    /// Different from `requirements` packages in this set are not necessarily included in the
    /// solution. Only when one or more packages list the package in their `requirements` is the
    /// package also added to the solution.
    ///
    /// This is often useful to use for optional dependencies.
    pub constrains: Vec<VersionSetId>,
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
