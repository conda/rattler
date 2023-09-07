use crate::id::VersionSetId;
use crate::id::{NameId, RepoId};

use std::fmt::{Display, Formatter};

/// A solvable that was derived from a Conda package
///
/// Contains a reference to the `PackageRecord` that corresponds to the solvable (the `'a` lifetime
/// comes from the original `PackageRecord`)
pub struct PackageSolvable<V> {
    pub(crate) repo_id: RepoId,
    pub(crate) dependencies: Vec<VersionSetId>,
    pub(crate) constrains: Vec<VersionSetId>,
    pub(crate) inner: V,
    pub(crate) name: NameId,
}

impl<V> PackageSolvable<V> {
    /// Returns the [`RepoId`] associated to this solvable
    pub fn repo_id(&self) -> RepoId {
        self.repo_id
    }

    /// Gets the record associated to this solvable
    pub fn inner(&self) -> &V {
        &self.inner
    }

    /// Get the dependencies for this solvable
    pub fn dependencies(&self) -> &[VersionSetId] {
        &self.dependencies
    }

    /// Get the constrains for this solvable
    pub fn constrains(&self) -> &[VersionSetId] {
        &self.constrains
    }

    /// Returns the name of the solvable
    pub fn name_id(&self) -> NameId {
        self.name
    }
}

/// Represents a package that can be installed
pub(crate) struct Solvable<V> {
    pub(crate) inner: SolvableInner<V>,
}

/// The inner representation of a solvable, which can be either a Conda package or the root solvable
pub(crate) enum SolvableInner<V> {
    Root(Vec<VersionSetId>),
    Package(PackageSolvable<V>),
}

impl<V> Solvable<V> {
    pub(crate) fn new_root() -> Self {
        Solvable {
            inner: SolvableInner::Root(Vec::new()),
        }
    }

    pub(crate) fn new_package(repo_id: RepoId, name: NameId, record: V) -> Self {
        Self {
            inner: SolvableInner::Package(PackageSolvable {
                repo_id,
                inner: record,
                name,
                dependencies: Vec::new(),
                constrains: Vec::new(),
            }),
        }
    }

    pub(crate) fn root_mut(&mut self) -> &mut Vec<VersionSetId> {
        match &mut self.inner {
            SolvableInner::Root(match_specs) => match_specs,
            _ => panic!("unexpected package solvable!"),
        }
    }

    pub(crate) fn get_package(&self) -> Option<&PackageSolvable<V>> {
        match &self.inner {
            SolvableInner::Root(_) => None,
            SolvableInner::Package(p) => Some(p),
        }
    }

    pub(crate) fn get_package_mut(&mut self) -> Option<&mut PackageSolvable<V>> {
        match &mut self.inner {
            SolvableInner::Root(_) => None,
            SolvableInner::Package(p) => Some(p),
        }
    }

    pub fn package(&self) -> &PackageSolvable<V> {
        self.get_package().expect("unexpected root solvable")
    }

    pub fn package_mut(&mut self) -> &mut PackageSolvable<V> {
        self.get_package_mut().expect("unexpected root solvable")
    }
}

impl<V: Display> Display for Solvable<V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            SolvableInner::Root(_) => write!(f, "<root>"),
            SolvableInner::Package(p) => write!(f, "{}", &p.inner),
        }
    }
}
