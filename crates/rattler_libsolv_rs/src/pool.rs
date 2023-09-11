use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};

use crate::arena::Arena;
use crate::id::{NameId, SolvableId, VersionSetId};
use crate::mapping::Mapping;
use crate::solvable::{PackageSolvable, Solvable};
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
    pub(crate) names_to_ids: HashMap<N, NameId>,

    /// Map from interned package names to the solvables that have that name
    pub(crate) packages_by_name: Mapping<NameId, Vec<SolvableId>>,

    /// Interned match specs
    pub(crate) version_sets: Arena<VersionSetId, (NameId, VS)>,

    /// Map from match spec strings to the id of their interned counterpart
    version_set_to_id: HashMap<(NameId, VS), VersionSetId>,

    /// Cached candidates for each match spec
    pub(crate) match_spec_to_sorted_candidates: Mapping<VersionSetId, Vec<SolvableId>>,

    /// Cached forbidden solvables for each match spec
    pub(crate) match_spec_to_forbidden: Mapping<VersionSetId, Vec<SolvableId>>,
}

impl<VS: VersionSet, N: PackageName> Default for Pool<VS, N> {
    fn default() -> Self {
        let mut solvables = Arena::new();
        solvables.alloc(Solvable::new_root());

        Self {
            solvables,

            names_to_ids: Default::default(),
            package_names: Arena::new(),
            packages_by_name: Mapping::empty(),

            version_set_to_id: Default::default(),
            version_sets: Arena::new(),
            match_spec_to_sorted_candidates: Mapping::empty(),
            match_spec_to_forbidden: Mapping::empty(),
        }
    }
}

impl<VS: VersionSet, N: PackageName> Pool<VS, N> {
    /// Creates a new [`Pool`]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a package to a repo and returns it's [`SolvableId`]
    pub fn add_package(&mut self, name_id: NameId, record: VS::V) -> SolvableId {
        assert!(self.solvables.len() <= u32::MAX as usize);

        let solvable_id = self.solvables.alloc(Solvable::new_package(name_id, record));

        self.packages_by_name[name_id].push(solvable_id);

        solvable_id
    }

    /// Registers a dependency for the provided solvable
    pub fn add_dependency(&mut self, solvable_id: SolvableId, version_set_id: VersionSetId) {
        let solvable = self.solvables[solvable_id].package_mut();
        solvable.dependencies.push(version_set_id);
    }

    /// Registers a constrains for the provided solvable
    pub fn add_constrains(&mut self, solvable_id: SolvableId, version_set_id: VersionSetId) {
        let solvable = self.solvables[solvable_id].package_mut();
        solvable.constrains.push(version_set_id);
    }

    /// Interns a match spec into the [`Pool`], returning its [`VersionSetId`]
    pub fn intern_version_set(&mut self, package_name: NameId, version_set: VS) -> VersionSetId {
        match self
            .version_set_to_id
            .entry((package_name, version_set.clone()))
        {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                let id = self.version_sets.alloc((package_name, version_set));

                // Update the entry
                entry.insert(id);

                id
            }
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
    pub fn intern_package_name<NValue>(&mut self, name: NValue) -> NameId
    where
        NValue: Into<N>,
        N: Clone,
    {
        match self.names_to_ids.entry(name.into()) {
            Entry::Occupied(e) => *e.get(),
            Entry::Vacant(e) => {
                let next_id = self.package_names.alloc(e.key().clone());

                // Keep the mapping in sync
                self.packages_by_name.extend(Vec::new());

                e.insert(next_id);
                next_id
            }
        }
    }

    /// Lookup the package name id associated to the provided name
    pub fn lookup_package_name(&self, name: &N) -> Option<NameId> {
        self.names_to_ids.get(name).copied()
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

    /// Finds all the solvables that match the specified version set.
    pub fn find_matching_solvables(&self, version_set_id: VersionSetId) -> Vec<SolvableId> {
        let (name_id, version_set) = &self.version_sets[version_set_id];

        self.packages_by_name[*name_id]
            .iter()
            .cloned()
            .filter(|&solvable| version_set.contains(self.solvables[solvable].package().inner()))
            .collect()
    }

    /// Finds all the solvables that do not match the specified version set.
    pub fn find_unmatched_solvables(&self, version_set_id: VersionSetId) -> Vec<SolvableId> {
        let (name_id, version_set) = &self.version_sets[version_set_id];

        self.packages_by_name[*name_id]
            .iter()
            .cloned()
            .filter(|&solvable| !version_set.contains(self.solvables[solvable].package().inner()))
            .collect()
    }

    /// Returns the solvable associated to the provided id
    ///
    /// Panics if the solvable is not found in the pool
    pub(crate) fn resolve_solvable_inner(&self, id: SolvableId) -> &Solvable<VS::V> {
        &self.solvables[id]
    }

    /// Returns the dependencies associated to the root solvable
    pub(crate) fn root_solvable_mut(&mut self) -> &mut Vec<VersionSetId> {
        self.solvables[SolvableId::root()].root_mut()
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
