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
use std::cell::OnceCell;
use std::fmt::{Debug, Display};
use std::hash::Hash;
pub use transaction::Transaction;

pub use mapping::Mapping;

use rattler_conda_types::MatchSpec;

/// Version is a name and a version specification.
pub trait VersionTrait: Display {
    /// The name of the package associated with this record.
    /// TODO: Can we move this to the Pool?
    type Name: Display + Sized + Hash + Eq + Clone;

    /// The version associated with this record.
    type Version: Display + Ord + Clone;

    /// Returns the version associated with this record
    // TODO: Can we get rid of this?
    fn version(&self) -> Self::Version;
}

/// Trait describing sets of versions.
pub trait VersionSet: Debug + Display + Clone + Eq + Hash {
    /// Version type associated with the sets manipulated.
    type V: VersionTrait;

    /// Evaluate membership of a version in this set.
    fn contains(&self, v: &Self::V) -> bool;
}

impl VersionSet for MatchSpec {
    type V = PackageRecord;

    fn contains(&self, v: &Self::V) -> bool {
        self.matches(v)
    }
}

impl VersionTrait for PackageRecord {
    type Name = String;
    type Version = rattler_conda_types::Version;

    fn version(&self) -> Self::Version {
        self.version.version().clone()
    }
}


/// TODO: Make this more generic, maybe even a generic <From, To> cache or something
/// like axum with any
pub trait SortCache {
    /// Initialize the cache with a specific size
    fn init_with_size(size: usize) -> Self;
}


/// Bla
pub trait DependencyProvider<VS: VersionSet> {
    /// Potential cache used when sorting candidates with each other
    type SortingCache: Default + SortCache;
    /// Sort the specified solvables based on which solvable to try first.
    fn sort_candidates(
        &self,
        pool: &Pool<VS>,
        solvables: &mut [SolvableId],
        match_spec_to_candidates: &Mapping<VersionSetId, OnceCell<Vec<SolvableId>>>,
        sort_cache: &Self::SortingCache
    );
}
/// Dependency provider for conda
pub struct CondaDependencyProvider;
/// Used when sorting conda candidates
#[derive(Default)]
pub struct CondaSortCache {
    match_spec_to_highest_version: Mapping<VersionSetId, OnceCell<Option<(rattler_conda_types::Version, bool)>>>,
}

impl SortCache for CondaSortCache {
    fn init_with_size(size: usize) -> Self {
        Self {
            match_spec_to_highest_version: Mapping::new(vec![OnceCell::new(); size]),
        }
    }
}

impl DependencyProvider<MatchSpec> for CondaDependencyProvider {
    type SortingCache = CondaSortCache;
    fn sort_candidates(
        &self,
        pool: &Pool<MatchSpec>,
        solvables: &mut [SolvableId],
        match_spec_to_candidates: &Mapping<VersionSetId, OnceCell<Vec<SolvableId>>>,
        sort_cache: &Self::SortingCache,
    ) {
        let match_spec_highest_version = &sort_cache.match_spec_to_highest_version;
        solvables.sort_by(|&p1, &p2| {
            conda_util::compare_candidates(
                p1,
                p2,
                pool,
                match_spec_to_candidates,
                match_spec_highest_version,
            )
        });
    }
}
