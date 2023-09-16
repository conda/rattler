use crate::arena::ArenaId;
use crate::solvable::DisplaySolvable;
use crate::{PackageName, Pool, VersionSet};
use std::fmt::Display;

/// The id associated to a package name
#[repr(transparent)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct NameId(u32);

impl ArenaId for NameId {
    fn from_usize(x: usize) -> Self {
        Self(x as u32)
    }

    fn to_usize(self) -> usize {
        self.0 as usize
    }
}

/// The id associated with a VersionSet.
#[repr(transparent)]
#[derive(Clone, Default, Copy, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct VersionSetId(u32);

impl ArenaId for VersionSetId {
    fn from_usize(x: usize) -> Self {
        Self(x as u32)
    }

    fn to_usize(self) -> usize {
        self.0 as usize
    }
}

/// The id associated to a solvable
#[repr(transparent)]
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
pub struct SolvableId(u32);

impl SolvableId {
    pub(crate) fn root() -> Self {
        Self(0)
    }

    pub(crate) fn is_root(self) -> bool {
        self.0 == 0
    }

    pub(crate) fn null() -> Self {
        Self(u32::MAX)
    }

    pub(crate) fn is_null(self) -> bool {
        self.0 == u32::MAX
    }

    /// Returns an object that enables formatting the solvable.
    pub fn display<VS: VersionSet, N: PackageName + Display>(
        self,
        pool: &Pool<VS, N>,
    ) -> DisplaySolvable<VS, N> {
        pool.resolve_internal_solvable(self).display(pool)
    }
}

impl ArenaId for SolvableId {
    fn from_usize(x: usize) -> Self {
        Self(x as u32)
    }

    fn to_usize(self) -> usize {
        self.0 as usize
    }
}

impl From<SolvableId> for u32 {
    fn from(value: SolvableId) -> Self {
        value.0
    }
}

#[repr(transparent)]
#[derive(Copy, Clone, PartialOrd, Ord, Eq, PartialEq, Debug, Hash)]
pub(crate) struct ClauseId(u32);

impl ClauseId {
    /// There is a guarentee that ClauseId(0) will always be "Clause::InstallRoot". This assumption
    /// is verified by the solver.
    pub(crate) fn install_root() -> Self {
        Self(0)
    }

    pub(crate) fn is_null(self) -> bool {
        self.0 == u32::MAX
    }

    pub(crate) fn null() -> ClauseId {
        ClauseId(u32::MAX)
    }
}

impl ArenaId for ClauseId {
    fn from_usize(x: usize) -> Self {
        assert!(x < u32::MAX as usize, "clause id too big");
        Self(x as u32)
    }

    fn to_usize(self) -> usize {
        self.0 as usize
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct LearntClauseId(u32);

impl ArenaId for LearntClauseId {
    fn from_usize(x: usize) -> Self {
        Self(x as u32)
    }

    fn to_usize(self) -> usize {
        self.0 as usize
    }
}

/// The id associated to an arena of Candidates
#[repr(transparent)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CandidatesId(u32);

impl ArenaId for CandidatesId {
    fn from_usize(x: usize) -> Self {
        Self(x as u32)
    }

    fn to_usize(self) -> usize {
        self.0 as usize
    }
}

/// The id associated to an arena of PackageRequirements
#[repr(transparent)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DependenciesId(u32);

impl ArenaId for DependenciesId {
    fn from_usize(x: usize) -> Self {
        Self(x as u32)
    }

    fn to_usize(self) -> usize {
        self.0 as usize
    }
}
