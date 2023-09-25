use crate::internal::id::NameId;

use crate::{PackageName, Pool, VersionSet};
use std::fmt::{Display, Formatter};

/// A solvable represents a single candidate of a package.
/// This is type is generic on `V` which can be supplied by the user. In most cases this is going
/// to be something like a record that contains the version of the package and other metadata.
/// A solvable is always associated with a [`NameId`], which is an interned name in the [`Pool`].
pub struct Solvable<V> {
    pub(crate) inner: V,
    pub(crate) name: NameId,
}

impl<V> Solvable<V> {
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
pub(crate) struct InternalSolvable<V> {
    pub(crate) inner: SolvableInner<V>,
}

/// The inner representation of a solvable, which can be either a Conda package or the root solvable
pub(crate) enum SolvableInner<V> {
    Root,
    Package(Solvable<V>),
}

impl<V> InternalSolvable<V> {
    pub(crate) fn new_root() -> Self {
        InternalSolvable {
            inner: SolvableInner::Root,
        }
    }

    pub(crate) fn new_solvable(name: NameId, record: V) -> Self {
        Self {
            inner: SolvableInner::Package(Solvable {
                inner: record,
                name,
            }),
        }
    }

    pub(crate) fn get_solvable(&self) -> Option<&Solvable<V>> {
        match &self.inner {
            SolvableInner::Root => None,
            SolvableInner::Package(p) => Some(p),
        }
    }

    pub fn solvable(&self) -> &Solvable<V> {
        self.get_solvable().expect("unexpected root solvable")
    }

    pub fn display<'pool, VS: VersionSet<V = V>, N: PackageName + Display>(
        &'pool self,
        pool: &'pool Pool<VS, N>,
    ) -> DisplaySolvable<'pool, VS, N> {
        DisplaySolvable {
            pool,
            solvable: self,
        }
    }
}

pub struct DisplaySolvable<'pool, VS: VersionSet, N: PackageName> {
    pool: &'pool Pool<VS, N>,
    solvable: &'pool InternalSolvable<VS::V>,
}

impl<'pool, VS: VersionSet, N: PackageName + Display> Display for DisplaySolvable<'pool, VS, N> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.solvable.inner {
            SolvableInner::Root => write!(f, "<root>"),
            SolvableInner::Package(p) => {
                write!(f, "{}={}", self.pool.resolve_package_name(p.name), &p.inner)
            }
        }
    }
}
