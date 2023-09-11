use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};

use crate::arena::Arena;
use crate::id::{NameId, SolvableId, VersionSetId};
use crate::solvable::{PackageSolvable, Solvable};
use crate::{FrozenMap, PackageRequirements};
use crate::{PackageName, VersionSet};

/// A pool that stores data related to the available packages
///
/// Because it stores solvables, it contains references to `PackageRecord`s (the `'a` lifetime comes
/// from the original `PackageRecord`s)
pub struct Pool<VS: VersionSet, N: PackageName = String> {
    /// All the solvables that have been registered
    pub(crate) solvables: Arena<SolvableId, Solvable<VS::V>>,

    /// Interned package names
    package_names: Arena<NameId, N>,

    /// Map from package names to the id of their interned counterpart
    pub(crate) names_to_ids: FrozenMap<N, NameId>,

    /// Interned match specs
    pub(crate) version_sets: Arena<VersionSetId, (NameId, VS)>,

    /// Map from match spec strings to the id of their interned counterpart
    version_set_to_id: FrozenMap<(NameId, VS), VersionSetId>,

    pub(crate) match_spec_to_sorted_candidates: elsa::FrozenMap<VersionSetId, Vec<SolvableId>>,
    pub(crate) match_spec_to_candidates: elsa::FrozenMap<VersionSetId, Vec<SolvableId>>,
}

impl<VS: VersionSet, N: PackageName> Default for Pool<VS, N> {
    fn default() -> Self {
        let mut solvables = Arena::new();
        solvables.alloc(Solvable::new_root());

        Self {
            solvables,

            names_to_ids: Default::default(),
            package_names: Arena::new(),
            version_set_to_id: Default::default(),
            version_sets: Arena::new(),

            match_spec_to_candidates: Default::default(),
            match_spec_to_sorted_candidates: Default::default(),
        }
    }
}

impl<VS: VersionSet, N: PackageName> Pool<VS, N> {
    /// Creates a new [`Pool`]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a package to a repo and returns it's [`SolvableId`]
    pub fn add_package(&self, name_id: NameId, record: VS::V) -> SolvableId {
        assert!(self.solvables.len() <= u32::MAX as usize);

        let solvable_id = self.solvables.alloc(Solvable::new_package(name_id, record));

        solvable_id
    }

    /// Sets the requirements of a specific package. Returns the requirements as an error if the
    /// requirements for the package have already been set.
    pub fn set_requirements(
        &self,
        solvable_id: SolvableId,
        requirements: PackageRequirements,
    ) -> Result<(), PackageRequirements> {
        let solvable = self.solvables[solvable_id].package();
        solvable.requirements.set(requirements)
    }

    /// Interns a match spec into the [`Pool`], returning its [`VersionSetId`]
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

    /// Returns the match spec associated to the provided id
    ///
    /// Panics if the version set is not found in the pool
    pub fn resolve_version_set(&self, id: VersionSetId) -> &VS {
        &self.version_sets[id].1
    }

    /// Returns the package name associated with the given version spec id.
    ///
    /// Panics if the version set is not found in the pool
    pub fn resolve_version_set_package_name(&self, id: VersionSetId) -> NameId {
        self.version_sets[id].0
    }

    /// Interns a package name into the `Pool`, returning its `NameId`
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

    /// Lookup the package name id associated to the provided name
    pub fn lookup_package_name(&self, name: &N) -> Option<NameId> {
        self.names_to_ids.get_copy(name)
    }

    /// Returns the package name associated to the provided id
    ///
    /// Panics if the package name is not found in the pool
    pub fn resolve_package_name(&self, name_id: NameId) -> &N {
        &self.package_names[name_id]
    }

    /// Returns the solvable associated to the provided id
    ///
    /// Panics if the solvable is not found in the pool
    pub fn resolve_solvable(&self, id: SolvableId) -> &PackageSolvable<VS::V> {
        self.resolve_solvable_inner(id).package()
    }

    /// Returns the solvable associated to the provided id
    ///
    /// Panics if the solvable is not found in the pool
    pub(crate) fn resolve_solvable_inner(&self, id: SolvableId) -> &Solvable<VS::V> {
        &self.solvables[id]
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
