use crate::id::NameId;
use crate::id::VersionSetId;
use std::cell::OnceCell;

use std::fmt::{Display, Formatter};

/// A solvable that was derived from a Conda package
///
/// Contains a reference to the `PackageRecord` that corresponds to the solvable (the `'a` lifetime
/// comes from the original `PackageRecord`)
pub struct PackageSolvable<V> {
    pub(crate) inner: V,
    pub(crate) name: NameId,
    pub(crate) requirements: OnceCell<PackageRequirements>,
}

pub struct PackageRequirements {
    dependencies: Vec<VersionSetId>,
    constrains: Vec<VersionSetId>,
}

impl<V> PackageSolvable<V> {
    /// Gets the record associated to this solvable
    pub fn inner(&self) -> &V {
        &self.inner
    }

    /// Get the dependencies for this solvable. Returns `None` if the requirements for this package
    /// have not been set.
    pub fn dependencies(&self) -> Option<&[VersionSetId]> {
        self.requirements.get().map(|r| r.dependencies.as_slice())
    }

    /// Get the constrains for this solvable. Returns `None` if the requirements for this package
    /// have not been set.
    pub fn constrains(&self) -> Option<&[VersionSetId]> {
        self.requirements.get().map(|r| r.constrains.as_slice())
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
    Root,
    Package(PackageSolvable<V>),
}

impl<V> Solvable<V> {
    pub(crate) fn new_root() -> Self {
        Solvable {
            inner: SolvableInner::Root,
        }
    }

    pub(crate) fn new_package(name: NameId, record: V) -> Self {
        Self {
            inner: SolvableInner::Package(PackageSolvable {
                inner: record,
                name,
                requirements: Default::default(),
            }),
        }
    }

    pub(crate) fn get_package(&self) -> Option<&PackageSolvable<V>> {
        match &self.inner {
            SolvableInner::Root => None,
            SolvableInner::Package(p) => Some(p),
        }
    }

    pub(crate) fn get_package_mut(&mut self) -> Option<&mut PackageSolvable<V>> {
        match &mut self.inner {
            SolvableInner::Root => None,
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
            SolvableInner::Root => write!(f, "<root>"),
            SolvableInner::Package(p) => write!(f, "{}", &p.inner),
        }
    }
}
