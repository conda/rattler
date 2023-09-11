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
mod frozen_copy_map;

pub use id::{NameId, SolvableId, VersionSetId};
pub use pool::Pool;
pub use solvable::{PackageSolvable, PackageRequirements};
pub use solve_jobs::SolveJobs;
pub use solver::Solver;
use std::cell::OnceCell;
use std::fmt::{Debug, Display};
use std::hash::Hash;
pub use transaction::Transaction;

pub(crate) use frozen_copy_map::FrozenCopyMap;
pub use mapping::Mapping;

/// Blanket trait implementation for something that we consider a package name.
pub trait PackageName: Eq + Hash {}
impl<N: Eq + Hash> PackageName for N {}

/// Version is a name and a version specification.
pub trait VersionTrait: Display {
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
pub trait DependencyProvider<VS: VersionSet, N: PackageName = String> {
    /// Sort the specified solvables based on which solvable to try first.
    fn sort_candidates(
        &self,
        pool: &Pool<VS, N>,
        solvables: &mut [SolvableId],
        match_spec_to_candidates: &Mapping<VersionSetId, OnceCell<Vec<SolvableId>>>,
    );

    /// Returns a list of solvables that should be considered when a package with the given name is
    /// requested.
    ///
    /// Returns `None` if no such package exist.
    fn get_candidates(&self, pool: &Pool<VS, N>, name: NameId) -> Option<Candidates>;

    /// Returns the dependencies for the specified solvable.
    fn get_dependencies(&self, pool: &Pool<VS, N>, solvable: SolvableId) -> Dependencies;
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
