use crate::conda_util;
use crate::id::{MatchSpecId, NameId, RepoId, SolvableId};
use crate::solvable::{PackageSolvable, Solvable};
use rattler_conda_types::{MatchSpec, PackageRecord};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::str::FromStr;

/// A pool that stores data related to the available packages
///
/// Because it stores solvables, it contains references to `PackageRecord`s (the `'a` lifetime comes
/// from the original `PackageRecord`s)
pub struct Pool<'a> {
    /// All the solvables that have been registered
    pub(crate) solvables: Vec<Solvable<'a>>,

    /// The total amount of registered repos
    total_repos: u32,

    /// Interned package names
    package_names: Vec<String>,

    /// Map from package names to the id of their interned counterpart
    pub(crate) names_to_ids: HashMap<String, NameId>,

    /// Map from interned package names to the solvables that have that name
    pub(crate) packages_by_name: Vec<Vec<SolvableId>>,

    /// Interned match specs
    pub(crate) match_specs: Vec<MatchSpec>,

    /// Map from match spec strings to the id of their interned counterpart
    match_specs_to_ids: HashMap<String, MatchSpecId>,

    /// Cached candidates for each match spec, indexed by their MatchSpecId
    pub(crate) match_spec_to_candidates: Vec<Option<Vec<SolvableId>>>,

    /// Cached forbidden solvables for each match spec, indexed by their MatchSpecId
    pub(crate) match_spec_to_forbidden: Vec<Option<Vec<SolvableId>>>,
}

impl<'a> Default for Pool<'a> {
    fn default() -> Self {
        Self {
            solvables: vec![Solvable::new_root()],
            total_repos: 0,

            names_to_ids: HashMap::new(),
            package_names: Vec::new(),
            packages_by_name: Vec::new(),

            match_specs_to_ids: HashMap::default(),
            match_specs: Vec::new(),
            match_spec_to_candidates: Vec::new(),
            match_spec_to_forbidden: Vec::new(),
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

        let solvable_id = SolvableId::new(self.solvables.len());
        self.solvables
            .push(Solvable::new_package(repo_id, name, record));

        self.packages_by_name[name.index()].push(solvable_id);

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
        assert_ne!(self.solvables[solvable_id.index()].package().name, name);

        self.solvables[solvable_id.index()] = Solvable::new_package(repo_id, name, record);
    }

    /// Registers a dependency for the provided solvable
    pub fn add_dependency(&mut self, solvable_id: SolvableId, match_spec: String) {
        let match_spec_id = self.intern_matchspec(match_spec);
        let solvable = self.solvables[solvable_id.index()].package_mut();
        solvable.dependencies.push(match_spec_id);
    }

    /// Registers a constrains for the provided solvable
    pub fn add_constrains(&mut self, solvable_id: SolvableId, match_spec: String) {
        let match_spec_id = self.intern_matchspec(match_spec);
        let solvable = self.solvables[solvable_id.index()].package_mut();
        solvable.constrains.push(match_spec_id);
    }

    // This function does not take `self`, because otherwise we run into problems with borrowing
    // when we want to use it together with other pool functions
    pub(crate) fn get_candidates<'b>(
        match_specs: &[MatchSpec],
        strings_to_ids: &HashMap<String, NameId>,
        solvables: &[Solvable],
        packages_by_name: &[Vec<SolvableId>],
        match_spec_to_candidates: &'b mut [Option<Vec<SolvableId>>],
        favored_map: &HashMap<NameId, SolvableId>,
        match_spec_id: MatchSpecId,
    ) -> &'b [SolvableId] {
        let candidates = match_spec_to_candidates[match_spec_id.index()].get_or_insert_with(|| {
            let match_spec = &match_specs[match_spec_id.index()];
            let match_spec_name = match_spec
                .name
                .as_deref()
                .expect("match spec without name!");
            let name_id = match strings_to_ids.get(match_spec_name) {
                None => return Vec::new(),
                Some(name_id) => name_id,
            };

            let mut pkgs: Vec<_> = packages_by_name[name_id.index()]
                .iter()
                .cloned()
                .filter(|solvable| match_spec.matches(solvables[solvable.index()].package().record))
                .collect();

            pkgs.sort_by(|p1, p2| {
                conda_util::compare_candidates(
                    solvables,
                    strings_to_ids,
                    packages_by_name,
                    solvables[p1.index()].package().record,
                    solvables[p2.index()].package().record,
                )
            });

            if let Some(&favored_id) = favored_map.get(name_id) {
                if let Some(pos) = pkgs.iter().position(|&s| s == favored_id) {
                    let removed = pkgs.remove(pos);
                    pkgs.insert(0, removed);
                }
            }

            pkgs
        });

        candidates.as_slice()
    }

    // This function does not take `self`, because otherwise we run into problems with borrowing
    // when we want to use it together with other pool functions
    pub(crate) fn get_forbidden<'b>(
        match_specs: &[MatchSpec],
        strings_to_ids: &HashMap<String, NameId>,
        solvables: &[Solvable],
        packages_by_name: &[Vec<SolvableId>],
        match_spec_to_forbidden: &'b mut [Option<Vec<SolvableId>>],
        match_spec_id: MatchSpecId,
    ) -> &'b [SolvableId] {
        let candidates = match_spec_to_forbidden[match_spec_id.index()].get_or_insert_with(|| {
            let match_spec = &match_specs[match_spec_id.index()];
            let match_spec_name = match_spec
                .name
                .as_deref()
                .expect("match spec without name!");
            let name_id = match strings_to_ids.get(match_spec_name) {
                None => return Vec::new(),
                Some(name_id) => name_id,
            };

            packages_by_name[name_id.index()]
                .iter()
                .cloned()
                .filter(|solvable| {
                    !match_spec.matches(solvables[solvable.index()].package().record)
                })
                .collect()
        });

        candidates.as_slice()
    }

    /// Interns a match spec into the `Pool`, returning its `MatchSpecId`
    pub(crate) fn intern_matchspec(&mut self, match_spec: String) -> MatchSpecId {
        let next_index = self.match_specs.len();
        match self.match_specs_to_ids.entry(match_spec) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                // println!("Interning match_spec: {}", entry.key());
                self.match_specs
                    .push(MatchSpec::from_str(entry.key()).unwrap());
                self.match_spec_to_candidates.push(None);
                self.match_spec_to_forbidden.push(None);

                // Update the entry
                let id = MatchSpecId::new(next_index);
                entry.insert(id);

                id
            }
        }
    }

    /// Returns the match spec associated to the provided id
    ///
    /// Panics if the match spec is not found in the pool
    pub fn resolve_match_spec(&self, id: MatchSpecId) -> &MatchSpec {
        &self.match_specs[id.index()]
    }

    /// Interns a package name into the `Pool`, returning its `NameId`
    fn intern_package_name<T: Into<String>>(&mut self, str: T) -> NameId {
        let next_id = NameId::new(self.names_to_ids.len());
        match self.names_to_ids.entry(str.into()) {
            Entry::Occupied(e) => *e.get(),
            Entry::Vacant(e) => {
                self.package_names.push(e.key().clone());
                self.packages_by_name.push(Vec::new());
                e.insert(next_id);
                next_id
            }
        }
    }

    /// Returns the package name associated to the provided id
    ///
    /// Panics if the package name is not found in the pool
    pub fn resolve_package_name(&self, name_id: NameId) -> &str {
        &self.package_names[name_id.index()]
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
        if id.index() < self.solvables.len() {
            &self.solvables[id.index()]
        } else {
            panic!("invalid solvable id!")
        }
    }

    /// Returns the solvable associated to the provided id
    ///
    /// Panics if the solvable is not found in the pool
    pub(crate) fn resolve_solvable_inner_mut(&mut self, id: SolvableId) -> &mut Solvable<'a> {
        if id.index() < self.solvables.len() {
            &mut self.solvables[id.index()]
        } else {
            panic!("invalid solvable id!")
        }
    }

    /// Returns the dependencies associated to the root solvable
    pub(crate) fn root_solvable_mut(&mut self) -> &mut Vec<MatchSpecId> {
        self.solvables[0].root_mut()
    }
}
