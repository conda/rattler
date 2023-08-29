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
use rattler_conda_types::{PackageRecord, Version};
pub use solvable::{PackageSolvable, SolvableMetadata};
pub use solve_jobs::SolveJobs;
pub use solver::Solver;
use std::cell::OnceCell;
use std::fmt::{Debug, Display};
use std::hash::Hash;
pub use transaction::Transaction;

pub use mapping::Mapping;

use rattler_conda_types::MatchSpec;

/// Record is a name and a version specification.
pub trait Record: Display {
    /// The name of the package associated with this record.
    type Name: Display + Sized + Hash + Eq + Clone;

    /// The version associated with this record.
    type Version: Display + Ord;

    /// Returns the package name associated with this record
    // TODO: Can we get rid of this?
    fn name(&self) -> Self::Name;

    /// Returns the version associated with this record
    // TODO: Can we get rid of this?
    fn version(&self) -> Self::Version;
}

/// Trait describing sets of versions.
pub trait VersionSet: Debug + Display + Clone + Eq + Hash {
    /// Version type associated with the sets manipulated.
    type V: Record;

    /// Evaluate membership of a version in this set.
    fn contains(&self, v: &Self::V) -> bool;
}

impl VersionSet for MatchSpec {
    type V = PackageRecord;

    fn contains(&self, v: &Self::V) -> bool {
        self.matches(v)
    }
}

impl Record for PackageRecord {
    type Name = String;
    type Version = rattler_conda_types::Version;
    fn name(&self) -> Self::Name {
        self.name.as_normalized().to_string()
    }

    fn version(&self) -> Self::Version {
        self.version.version().clone()
    }
}

/// Bla
pub trait DependencyProvider<VS: VersionSet> {
    /// Sort the specified solvables based on which solvable to try first.
    fn sort_candidates(
        &self,
        pool: &Pool<VS>,
        solvables: &mut [SolvableId],
        match_spec_to_candidates: &Mapping<VersionSetId, OnceCell<Vec<SolvableId>>>,
        match_spec_highest_version: &Mapping<VersionSetId, OnceCell<Option<(Version, bool)>>>,
    );
}
/// Dependency provider fro conda
pub struct CondaDependencyProvider;

impl DependencyProvider<MatchSpec> for CondaDependencyProvider {
    fn sort_candidates(
        &self,
        pool: &Pool<MatchSpec>,
        solvables: &mut [SolvableId],
        match_spec_to_candidates: &Mapping<VersionSetId, OnceCell<Vec<SolvableId>>>,
        match_spec_highest_version: &Mapping<VersionSetId, OnceCell<Option<(Version, bool)>>>,
    ) {
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
