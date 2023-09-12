use crate::id::NameId;

use std::fmt::{Display, Formatter};

/// A solvable that was derived from a Conda package
///
/// Contains a reference to the `PackageRecord` that corresponds to the solvable (the `'a` lifetime
/// comes from the original `PackageRecord`)
pub struct PackageSolvable<V> {
    pub(crate) inner: V,
    pub(crate) name: NameId,
}

impl<V> PackageSolvable<V> {
    /// Gets the record associated to this solvable
    pub fn inner(&self) -> &V {
        &self.inner
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
            }),
        }
    }

    pub(crate) fn get_package(&self) -> Option<&PackageSolvable<V>> {
        match &self.inner {
            SolvableInner::Root => None,
            SolvableInner::Package(p) => Some(p),
        }
    }

    pub fn package(&self) -> &PackageSolvable<V> {
        self.get_package().expect("unexpected root solvable")
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
