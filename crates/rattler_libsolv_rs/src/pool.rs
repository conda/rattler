use std::fmt::{Display, Formatter};

use crate::arena::Arena;
use crate::id::{NameId, SolvableId, VersionSetId};
use crate::solvable::{InternalSolvable, Solvable};
use crate::FrozenCopyMap;
use crate::{PackageName, VersionSet};

/// A pool that stores data related to the available packages.
///
/// A pool never releases its memory until it is dropped. References returned by the pool will
/// remain valid for the lifetime of the pool. This allows inserting into the pool without requiring
/// a mutable reference to the pool.
pub struct Pool<VS: VersionSet, N: PackageName = String> {
    /// All the solvables that have been registered
    pub(crate) solvables: Arena<SolvableId, InternalSolvable<VS::V>>,

    /// Interned package names
    package_names: Arena<NameId, N>,

    /// Map from package names to the id of their interned counterpart
    pub(crate) names_to_ids: FrozenCopyMap<N, NameId>,

    /// Interned match specs
    pub(crate) version_sets: Arena<VersionSetId, (NameId, VS)>,

    /// Map from version set to the id of their interned counterpart
    version_set_to_id: FrozenCopyMap<(NameId, VS), VersionSetId>,
}

impl<VS: VersionSet, N: PackageName> Default for Pool<VS, N> {
    fn default() -> Self {
        let solvables = Arena::new();
        solvables.alloc(InternalSolvable::new_root());

        Self {
            solvables,

            names_to_ids: Default::default(),
            package_names: Arena::new(),
            version_set_to_id: Default::default(),
            version_sets: Arena::new(),
        }
    }
}

impl<VS: VersionSet, N: PackageName> Pool<VS, N> {
    /// Creates a new [`Pool`]
    pub fn new() -> Self {
        Self::default()
    }

    /// Interns a package name into the `Pool`, returning its `NameId`. Names are deduplicated. If
    /// the same name is inserted twice the same `NameId` will be returned.
    ///
    /// The original name can be resolved using the [`Self::resolve_package_name`] function.
    pub fn intern_package_name<NValue>(&self, name: NValue) -> NameId
    where
        NValue: Into<N>,
        N: Clone,
    {
        let name = name.into();
        if let Some(id) = self.names_to_ids.get_copy(&name) {
            return id;
        }

        let next_id = self.package_names.alloc(name.clone());
        self.names_to_ids.insert_copy(name, next_id);
        next_id
    }

    /// Returns the package name associated with the provided [`NameId`].
    ///
    /// Panics if the package name is not found in the pool.
    pub fn resolve_package_name(&self, name_id: NameId) -> &N {
        &self.package_names[name_id]
    }

    /// Returns the [`NameId`] associated with the specified name or `None` if the name has not
    /// previously been interned using [`Self::intern_package_name`].
    pub fn lookup_package_name(&self, name: &N) -> Option<NameId> {
        self.names_to_ids.get_copy(name)
    }

    /// Adds a solvable to a repo and returns it's [`SolvableId`].
    ///
    /// Unlike some of the other interning functions this function does *not* deduplicate any of the
    /// inserted elements. A unique Id will be returned everytime this function is called.
    pub fn intern_solvable(&self, name_id: NameId, record: VS::V) -> SolvableId {
        self.solvables
            .alloc(InternalSolvable::new_solvable(name_id, record))
    }

    /// Returns the solvable associated to the provided id
    ///
    /// Panics if the solvable is not found in the pool
    pub fn resolve_solvable(&self, id: SolvableId) -> &Solvable<VS::V> {
        self.resolve_internal_solvable(id).solvable()
    }

    /// Returns the solvable associated to the provided id
    ///
    /// Panics if the solvable is not found in the pool
    pub(crate) fn resolve_internal_solvable(&self, id: SolvableId) -> &InternalSolvable<VS::V> {
        &self.solvables[id]
    }

    /// Interns a version set into the [`Pool`], returning its [`VersionSetId`]. The returned
    /// [`VersionSetId`] can be used to retrieve a reference to the original version set using
    /// [`Self::resolve_version-set`].
    ///
    /// A version set is always associated with a specific package name to which it applies. The
    /// passed in package name can be retrieved using [`Self::resolve_version_set_package_name`].
    ///
    /// Version sets are deduplicated. This means that if the same version set is inserted twice
    /// they will share the same [`VersionSetId`].
    pub fn intern_version_set(&self, package_name: NameId, version_set: VS) -> VersionSetId {
        if let Some(entry) = self
            .version_set_to_id
            .get_copy(&(package_name, version_set.clone()))
        {
            entry
        } else {
            let id = self.version_sets.alloc((package_name, version_set.clone()));
            self.version_set_to_id
                .insert_copy((package_name, version_set), id);
            id
        }
    }

    /// Returns the version set associated with the provided id
    ///
    /// Panics if the version set is not found in the pool
    pub fn resolve_version_set(&self, id: VersionSetId) -> &VS {
        &self.version_sets[id].1
    }

    /// Returns the package name associated with the provide id.
    ///
    /// Panics if the version set is not found in the pool
    pub fn resolve_version_set_package_name(&self, id: VersionSetId) -> NameId {
        self.version_sets[id].0
    }
}

/// A helper struct to visualize a name.
pub struct NameDisplay<'pool, VS: VersionSet, N: PackageName> {
    id: NameId,
    pool: &'pool Pool<VS, N>,
}

impl<'pool, VS: VersionSet, N: PackageName + Display> Display for NameDisplay<'pool, VS, N> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let name = self.pool.resolve_package_name(self.id);
        write!(f, "{}", name)
    }
}

impl NameId {
    /// Returns an object that can be used to format the name.
    pub fn display<VS: VersionSet, N: PackageName + Display>(
        self,
        pool: &Pool<VS, N>,
    ) -> NameDisplay<'_, VS, N> {
        NameDisplay { id: self, pool }
    }
}
