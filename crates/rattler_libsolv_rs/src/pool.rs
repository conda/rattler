use crate::arena::Arena;
use crate::conda_util;
use crate::id::{VersionSetId, NameId, RepoId, SolvableId};
use crate::mapping::Mapping;
use crate::solvable::{PackageSolvable, Solvable};
use rattler_conda_types::{MatchSpec, PackageName, PackageRecord, Version};
use std::cell::OnceCell;
use std::cmp::Ordering;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::hash::Hash;

/// Trait describing sets of versions.
pub trait VersionSet: Debug + Display + Clone + Eq + Hash  {
    type V: Debug + Display + Clone + Eq + Ord;

    /// Evaluate membership of a version in this set.
    fn contains(&self, v: &Self::V) -> bool;

}

impl VersionSet for MatchSpec {
    type V = PackageRecord;

    fn contains(&self, v: &Self::V) -> bool {
        self.matches(v)
    }
}


/// A pool that stores data related to the available packages
///
/// Because it stores solvables, it contains references to `PackageRecord`s (the `'a` lifetime comes
/// from the original `PackageRecord`s)
pub struct Pool<'a, V: VersionSet> {
    /// All the solvables that have been registered
    pub(crate) solvables: Arena<SolvableId, Solvable<'a>>,

    /// The total amount of registered repos
    total_repos: u32,

    /// Interned package names
    package_names: Arena<NameId, PackageName>,

    /// Map from package names to the id of their interned counterpart
    pub(crate) names_to_ids: HashMap<PackageName, NameId>,

    /// Map from interned package names to the solvables that have that name
    pub(crate) packages_by_name: Mapping<NameId, Vec<SolvableId>>,

    /// Interned match specs
    pub(crate) version_sets: Arena<VersionSetId, V>,

    /// Map from match spec strings to the id of their interned counterpart
    version_set_to_id: HashMap<V, VersionSetId>,

    /// Cached candidates for each match spec
    pub(crate) match_spec_to_sorted_candidates: Mapping<VersionSetId, Vec<SolvableId>>,

    /// Cached forbidden solvables for each match spec
    pub(crate) match_spec_to_forbidden: Mapping<VersionSetId, Vec<SolvableId>>,
}

impl<'a, V: VersionSet> Default for Pool<'a, V> {
    fn default() -> Self {
        let mut solvables = Arena::new();
        solvables.alloc(Solvable::new_root());

        Self {
            solvables,
            total_repos: 0,

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

impl<'a, V: VersionSet> Pool<'a, V> {
    /// Creates a new [`Pool`]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a new repo (i.e. a source of packages)
    pub fn new_repo(&mut self) -> RepoId {
        let id = RepoId::new(self.total_repos);
        self.total_repos += 1;
        id
    }

    /// Adds a package to a repo and returns it's [`SolvableId`]
    pub fn add_package(&mut self, repo_id: RepoId, record: &'a PackageRecord) -> SolvableId {
        assert!(self.solvables.len() <= u32::MAX as usize);

        let name = self.intern_package_name(&record.name);

        let solvable_id = self
            .solvables
            .alloc(Solvable::new_package(repo_id, name, record));

        self.packages_by_name[name].push(solvable_id);

        solvable_id
    }

    /// Overwrites the package associated to the id, as though it had just been created using
    /// [`Pool::add_package`]
    ///
    /// Panics if the new package has a different name than the existing package
    pub fn overwrite_package(
        &mut self,
        repo_id: RepoId,
        solvable_id: SolvableId,
        record: &'a PackageRecord,
    ) {
        assert!(!solvable_id.is_root());

        let name = self.intern_package_name(&record.name);
        assert_eq!(self.solvables[solvable_id].package().name, name);

        self.solvables[solvable_id] = Solvable::new_package(repo_id, name, record);
    }

    /// Registers a dependency for the provided solvable
    pub fn add_dependency(&mut self, solvable_id: SolvableId, version_set: V) {
        let match_spec_id = self.intern_version_set(version_set);
        let solvable = self.solvables[solvable_id].package_mut();
        solvable.dependencies.push(match_spec_id);
    }

    /// Registers a constrains for the provided solvable
    pub fn add_constrains(&mut self, solvable_id: SolvableId, version_set: V) {
        let match_spec_id = self.intern_version_set(version_set);
        let solvable = self.solvables[solvable_id].package_mut();
        solvable.constrains.push(match_spec_id);
    }

    /// Interns a match spec into the `Pool`, returning its `MatchSpecId`
    pub(crate) fn intern_version_set(&mut self, version_set: V) -> VersionSetId {
        match self.version_set_to_id.entry(version_set.clone()) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                let id = self
                    .version_sets
                    .alloc(version_set);

                // Update the entry
                entry.insert(id);

                id
            }
        }
    }

    /// Returns the match spec associated to the provided id
    ///
    /// Panics if the match spec is not found in the pool
    pub fn resolve_version_set(&self, id: VersionSetId) -> &V {
        &self.version_sets[id]
    }

    /// Interns a package name into the `Pool`, returning its `NameId`
    fn intern_package_name(&mut self, name: &PackageName) -> NameId {
        match self.names_to_ids.entry(name.clone()) {
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

    /// Returns the package name associated to the provided id
    ///
    /// Panics if the package name is not found in the pool
    pub fn resolve_package_name(&self, name_id: NameId) -> &PackageName {
        &self.package_names[name_id]
    }

    /// Returns the solvable associated to the provided id
    ///
    /// Panics if the solvable is not found in the pool
    pub fn resolve_solvable(&self, id: SolvableId) -> &PackageSolvable {
        self.resolve_solvable_inner(id).package()
    }

    /// Returns the solvable associated to the provided id
    ///
    /// Panics if the solvable is not found in the pool
    pub fn resolve_solvable_mut(&mut self, id: SolvableId) -> &mut PackageSolvable<'a> {
        self.resolve_solvable_inner_mut(id).package_mut()
    }

    /// Returns the solvable associated to the provided id
    ///
    /// Panics if the solvable is not found in the pool
    pub(crate) fn resolve_solvable_inner(&self, id: SolvableId) -> &Solvable {
        &self.solvables[id]
    }

    /// Returns the solvable associated to the provided id
    ///
    /// Panics if the solvable is not found in the pool
    pub(crate) fn resolve_solvable_inner_mut(&mut self, id: SolvableId) -> &mut Solvable<'a> {
        &mut self.solvables[id]
    }

    /// Returns the dependencies associated to the root solvable
    pub(crate) fn root_solvable_mut(&mut self) -> &mut Vec<VersionSetId> {
        self.solvables[SolvableId::root()].root_mut()
    }
}

impl Pool<'_, MatchSpec> {
    /// Populates the list of candidates for the provided match spec
    pub(crate) fn populate_candidates(
        &self,
        match_spec_id: VersionSetId,
        favored_map: &HashMap<NameId, SolvableId>,
        match_spec_to_sorted_candidates: &mut Mapping<VersionSetId, Vec<SolvableId>>,
        match_spec_to_candidates: &Mapping<VersionSetId, OnceCell<Vec<SolvableId>>>,
        match_spec_highest_version: &Mapping<VersionSetId, OnceCell<Option<(Version, bool)>>>,
        solvable_order: &mut HashMap<u64, Ordering>,
    ) {
        let match_spec = &self.version_sets[match_spec_id];
        let match_spec_name = match_spec
            .name
            .as_ref()
            .expect("match spec without name!");
        let name_id = match self.names_to_ids.get(match_spec_name) {
            None => return,
            Some(&name_id) => name_id,
        };

        let mut pkgs = conda_util::find_candidates(
            match_spec_id,
            &self.version_sets,
            &self.names_to_ids,
            &self.packages_by_name,
            &self.solvables,
            match_spec_to_candidates,
        )
            .clone();

        pkgs.sort_by(|&p1, &p2| {
            let key = u32::from(p1) as u64 | ((u32::from(p2) as u64) << 32);
            *solvable_order.entry(key).or_insert_with(|| {
                conda_util::compare_candidates(
                    p1,
                    p2,
                    &self.solvables,
                    &self.names_to_ids,
                    &self.packages_by_name,
                    &self.version_sets,
                    match_spec_to_candidates,
                    match_spec_highest_version,
                )
            })
        });

        if let Some(&favored_id) = favored_map.get(&name_id) {
            if let Some(pos) = pkgs.iter().position(|&s| s == favored_id) {
                // Move the element at `pos` to the front of the array
                pkgs[0..=pos].rotate_right(1);
            }
        }

        match_spec_to_sorted_candidates[match_spec_id] = pkgs;
    }

    /// Populates the list of forbidden packages for the provided match spec
    pub(crate) fn populate_forbidden(
        &self,
        match_spec_id: VersionSetId,
        version_set_to_forbidden: &mut Mapping<VersionSetId, Vec<SolvableId>>,
    ) {
        let match_spec = &self.version_sets[match_spec_id];
        let match_spec_name = match_spec
            .name
            .as_ref()
            .expect("match spec without name!");
        let name_id = match self.names_to_ids.get(match_spec_name) {
            None => return,
            Some(&name_id) => name_id,
        };

        version_set_to_forbidden[match_spec_id] = self.packages_by_name[name_id]
            .iter()
            .cloned()
            .filter(|&solvable| !match_spec.matches(self.solvables[solvable].package().record))
            .collect();
    }
}