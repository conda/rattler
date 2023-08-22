use crate::arena::Arena;
use crate::conda_util;
use crate::id::{MatchSpecId, NameId, RepoId, SolvableId};
use crate::mapping::Mapping;
use crate::solvable::{PackageSolvable, Solvable};
use rattler_conda_types::{MatchSpec, PackageName, PackageRecord, Version};
use std::cell::OnceCell;
use std::cmp::Ordering;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::str::FromStr;

/// A pool that stores data related to the available packages
///
/// Because it stores solvables, it contains references to `PackageRecord`s (the `'a` lifetime comes
/// from the original `PackageRecord`s)
pub struct Pool<'a> {
    /// All the solvables that have been registered
    pub(crate) solvables: Arena<SolvableId, Solvable<'a>>,

    /// The total amount of registered repos
    total_repos: u32,

    /// Interned package names
    package_names: Arena<NameId, String>,

    /// Map from package names to the id of their interned counterpart
    pub(crate) names_to_ids: HashMap<String, NameId>,

    /// Map from interned package names to the solvables that have that name
    pub(crate) packages_by_name: Mapping<NameId, Vec<SolvableId>>,

    /// Interned match specs
    pub(crate) match_specs: Arena<MatchSpecId, MatchSpec>,

    /// Map from match spec strings to the id of their interned counterpart
    match_specs_to_ids: HashMap<String, MatchSpecId>,

    /// Cached candidates for each match spec
    pub(crate) match_spec_to_sorted_candidates: Mapping<MatchSpecId, Vec<SolvableId>>,

    /// Cached forbidden solvables for each match spec
    pub(crate) match_spec_to_forbidden: Mapping<MatchSpecId, Vec<SolvableId>>,
}

impl<'a> Default for Pool<'a> {
    fn default() -> Self {
        let mut solvables = Arena::new();
        solvables.alloc(Solvable::new_root());

        Self {
            solvables,
            total_repos: 0,

            names_to_ids: Default::default(),
            package_names: Arena::new(),
            packages_by_name: Mapping::empty(),

            match_specs_to_ids: Default::default(),
            match_specs: Arena::new(),
            match_spec_to_sorted_candidates: Mapping::empty(),
            match_spec_to_forbidden: Mapping::empty(),
        }
    }
}

impl<'a> Pool<'a> {
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
    pub fn add_dependency(&mut self, solvable_id: SolvableId, match_spec: String) {
        let match_spec_id = self.intern_matchspec(match_spec);
        let solvable = self.solvables[solvable_id].package_mut();
        solvable.dependencies.push(match_spec_id);
    }

    /// Registers a constrains for the provided solvable
    pub fn add_constrains(&mut self, solvable_id: SolvableId, match_spec: String) {
        let match_spec_id = self.intern_matchspec(match_spec);
        let solvable = self.solvables[solvable_id].package_mut();
        solvable.constrains.push(match_spec_id);
    }

    /// Populates the list of candidates for the provided match spec
    pub(crate) fn populate_candidates(
        &self,
        match_spec_id: MatchSpecId,
        favored_map: &HashMap<NameId, SolvableId>,
        match_spec_to_sorted_candidates: &mut Mapping<MatchSpecId, Vec<SolvableId>>,
        match_spec_to_candidates: &Mapping<MatchSpecId, OnceCell<Vec<SolvableId>>>,
        match_spec_highest_version: &Mapping<MatchSpecId, OnceCell<Option<(Version, bool)>>>,
        solvable_order: &mut HashMap<u64, Ordering>,
    ) {
        let match_spec = &self.match_specs[match_spec_id];
        let match_spec_name = match_spec.name.as_ref().expect("match spec without name!");
        let name_id = match self
            .names_to_ids
            .get(match_spec_name.as_normalized().as_ref())
        {
            None => return,
            Some(&name_id) => name_id,
        };

        let mut pkgs = conda_util::find_candidates(
            match_spec_id,
            &self.match_specs,
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
                    &self.match_specs,
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
        match_spec_id: MatchSpecId,
        match_spec_to_forbidden: &mut Mapping<MatchSpecId, Vec<SolvableId>>,
    ) {
        let match_spec = &self.match_specs[match_spec_id];
        let match_spec_name = match_spec.name.as_ref().expect("match spec without name!");
        let name_id = match self
            .names_to_ids
            .get(match_spec_name.as_normalized().as_ref())
        {
            None => return,
            Some(&name_id) => name_id,
        };

        match_spec_to_forbidden[match_spec_id] = self.packages_by_name[name_id]
            .iter()
            .cloned()
            .filter(|&solvable| !match_spec.matches(self.solvables[solvable].package().record))
            .collect();
    }

    /// Interns a match spec into the `Pool`, returning its `MatchSpecId`
    pub(crate) fn intern_matchspec(&mut self, match_spec: String) -> MatchSpecId {
        match self.match_specs_to_ids.entry(match_spec) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                let id = self
                    .match_specs
                    .alloc(MatchSpec::from_str(entry.key()).unwrap());

                // Update the entry
                entry.insert(id);

                id
            }
        }
    }

    /// Returns the match spec associated to the provided id
    ///
    /// Panics if the match spec is not found in the pool
    pub fn resolve_match_spec(&self, id: MatchSpecId) -> &MatchSpec {
        &self.match_specs[id]
    }

    /// Interns a package name into the `Pool`, returning its `NameId`
    fn intern_package_name<T: Into<PackageName>>(&mut self, name: T) -> NameId {
        let package_name = name.into();
        match self
            .names_to_ids
            .entry(package_name.as_normalized().to_string())
        {
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
    pub fn resolve_package_name(&self, name_id: NameId) -> &str {
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
    pub(crate) fn root_solvable_mut(&mut self) -> &mut Vec<MatchSpecId> {
        self.solvables[SolvableId::root()].root_mut()
    }
}
