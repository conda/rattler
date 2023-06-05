use crate::conda_util;
use crate::solvable::{PackageSolvable, Solvable, SolvableId};
use rattler_conda_types::{MatchSpec, PackageRecord};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::str::FromStr;

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct RepoId(u32);

impl RepoId {
    fn new(id: u32) -> Self {
        Self(id)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct StringId {
    value: u32,
}

impl StringId {
    pub(crate) fn new(index: usize) -> Self {
        Self {
            value: index as u32,
        }
    }

    pub(crate) fn index(self) -> usize {
        self.value as usize
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MatchSpecId(u32);

impl MatchSpecId {
    fn new(index: usize) -> Self {
        Self(index as u32)
    }

    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

pub struct Pool {
    pub(crate) solvables: Vec<Solvable>,

    /// The total amount of registered repos
    total_repos: u32,

    /// Interned strings
    pub(crate) strings_to_ids: HashMap<String, StringId>,
    strings: Vec<String>,

    /// Interned match specs
    match_specs_to_ids: HashMap<String, MatchSpecId>,
    pub(crate) match_specs: Vec<MatchSpec>,

    /// Cached candidates for each match spec, indexed by their MatchSpecId
    pub(crate) match_spec_to_candidates: Vec<Option<Vec<SolvableId>>>,

    pub(crate) match_spec_to_forbidden: Vec<Option<Vec<SolvableId>>>,

    // TODO: eventually we could turn this into a Vec, making sure we have a separate interning
    // scheme for package names
    pub(crate) packages_by_name: HashMap<StringId, Vec<SolvableId>>,
}

impl Default for Pool {
    fn default() -> Self {
        Self {
            solvables: vec![Solvable::new_root()],
            total_repos: 0,

            strings_to_ids: HashMap::new(),
            strings: Vec::new(),

            packages_by_name: HashMap::default(),

            match_specs_to_ids: HashMap::default(),
            match_specs: Vec::new(),
            match_spec_to_candidates: Vec::new(),
            match_spec_to_forbidden: Vec::new(),
        }
    }
}

impl Pool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_repo(&mut self, _url: impl AsRef<str>) -> RepoId {
        let id = RepoId::new(self.total_repos);
        self.total_repos += 1;
        id
    }

    /// Adds a new solvable to a repo
    pub fn add_package(&mut self, repo_id: RepoId, record: &'static PackageRecord) -> SolvableId {
        assert!(self.solvables.len() <= u32::MAX as usize);

        let name = self.intern_str(&record.name);

        let solvable_id = SolvableId::new(self.solvables.len());
        self.solvables
            .push(Solvable::new_package(repo_id, name, record));

        assert!(repo_id.0 < self.total_repos);

        self.packages_by_name
            .entry(name)
            .or_insert(Vec::new())
            .push(solvable_id);

        solvable_id
    }

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

    pub fn reset_package(
        &mut self,
        repo_id: RepoId,
        solvable_id: SolvableId,
        record: &'static PackageRecord,
    ) {
        let name = self.intern_str(&record.name);
        self.solvables[solvable_id.index()] = Solvable::new_package(repo_id, name, record);
    }

    // This function does not take `self`, because otherwise we run into problems with borrowing
    // when we want to use it together with other pool functions
    pub(crate) fn get_candidates<'a>(
        match_specs: &'a [MatchSpec],
        strings_to_ids: &'a HashMap<String, StringId>,
        solvables: &'a [Solvable],
        packages_by_name: &'a HashMap<StringId, Vec<SolvableId>>,
        match_spec_to_candidates: &'a mut [Option<Vec<SolvableId>>],
        favored_map: &HashMap<StringId, SolvableId>,
        match_spec_id: MatchSpecId,
    ) -> &'a [SolvableId] {
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

            let mut pkgs: Vec<_> = packages_by_name[name_id]
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
                    pkgs.swap(0, pos);
                }
            }

            pkgs
        });

        candidates.as_slice()
    }

    // This function does not take `self`, because otherwise we run into problems with borrowing
    // when we want to use it together with other pool functions
    pub(crate) fn get_forbidden<'a>(
        match_specs: &'a [MatchSpec],
        strings_to_ids: &'a HashMap<String, StringId>,
        solvables: &'a [Solvable],
        packages_by_name: &'a HashMap<StringId, Vec<SolvableId>>,
        match_spec_to_forbidden: &'a mut [Option<Vec<SolvableId>>],
        match_spec_id: MatchSpecId,
    ) -> &'a [SolvableId] {
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

            packages_by_name[name_id]
                .iter()
                .cloned()
                .filter(|solvable| {
                    !match_spec.matches(solvables[solvable.index()].package().record)
                })
                .collect()
        });

        candidates.as_slice()
    }

    pub fn add_dependency(&mut self, solvable_id: SolvableId, match_spec: String) {
        let match_spec_id = self.intern_matchspec(match_spec);
        let solvable = self.solvables[solvable_id.index()].package_mut();
        solvable.dependencies.push(match_spec_id);
    }

    pub fn add_constrains(&mut self, solvable_id: SolvableId, match_spec: String) {
        let match_spec_id = self.intern_matchspec(match_spec);
        let solvable = self.solvables[solvable_id.index()].package_mut();
        solvable.constrains.push(match_spec_id);
    }

    pub(crate) fn nsolvables(&self) -> u32 {
        self.solvables.len() as u32
    }

    /// Interns string like types into a `Pool` returning a `StringId`
    pub(crate) fn intern_str<T: Into<String>>(&mut self, str: T) -> StringId {
        let next_id = StringId::new(self.strings_to_ids.len());
        match self.strings_to_ids.entry(str.into()) {
            Entry::Occupied(e) => *e.get(),
            Entry::Vacant(e) => {
                self.strings.push(e.key().clone());
                e.insert(next_id);
                next_id
            }
        }
    }

    pub fn resolve_string(&self, string_id: StringId) -> &str {
        &self.strings[string_id.index()]
    }

    /// Returns a string describing the last error associated to this pool, or "no error" if there
    /// were no errors
    pub fn last_error(&self) -> String {
        // See pool_errstr
        "no error".to_string()
    }

    /// Resolves the id to a solvable
    ///
    /// Panics if the solvable is not found in the pool
    pub fn resolve_solvable(&self, id: SolvableId) -> &PackageSolvable {
        self.resolve_solvable_inner(id).package()
    }

    /// Resolves the id to a solvable
    ///
    /// Panics if the solvable is not found in the pool
    pub fn resolve_solvable_mut(&mut self, id: SolvableId) -> &mut PackageSolvable {
        self.resolve_solvable_inner_mut(id).package_mut()
    }

    /// Resolves the id to a solvable
    ///
    /// Panics if the solvable is not found in the pool
    pub(crate) fn resolve_solvable_inner(&self, id: SolvableId) -> &Solvable {
        if id.index() < self.solvables.len() {
            &self.solvables[id.index()]
        } else {
            panic!("invalid solvable id!")
        }
    }

    /// Resolves the id to a solvable
    ///
    /// Panics if the solvable is not found in the pool
    pub(crate) fn resolve_solvable_inner_mut(&mut self, id: SolvableId) -> &mut Solvable {
        if id.index() < self.solvables.len() {
            &mut self.solvables[id.index()]
        } else {
            panic!("invalid solvable id!")
        }
    }

    pub(crate) fn resolve_match_spec(&self, id: MatchSpecId) -> &MatchSpec {
        &self.match_specs[id.index()]
    }

    pub(crate) fn root_solvable_mut(&mut self) -> &mut Vec<MatchSpecId> {
        self.solvables[0].root_mut()
    }
}
