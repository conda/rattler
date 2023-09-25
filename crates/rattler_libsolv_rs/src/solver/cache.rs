use crate::internal::arena::ArenaId;
use crate::{
    internal::{
        arena::Arena,
        frozen_copy_map::FrozenCopyMap,
        id::{CandidatesId, DependenciesId},
    },
    Candidates, Dependencies, DependencyProvider, NameId, PackageName, Pool, SolvableId,
    VersionSet, VersionSetId,
};
use bitvec::vec::BitVec;
use elsa::FrozenMap;
use std::cell::RefCell;
use std::marker::PhantomData;

/// Keeps a cache of previously computed and/or requested information about solvables and version
/// sets.
pub struct SolverCache<VS: VersionSet, N: PackageName, D: DependencyProvider<VS, N>> {
    provider: D,

    /// A mapping from package name to a list of candidates.
    candidates: Arena<CandidatesId, Candidates>,
    package_name_to_candidates: FrozenCopyMap<NameId, CandidatesId>,

    /// A mapping of `VersionSetId` to the candidates that match that set.
    version_set_candidates: FrozenMap<VersionSetId, Vec<SolvableId>>,

    /// A mapping of `VersionSetId` to the candidates that do not match that set (only candidates
    /// of the package indicated by the version set are included).
    version_set_inverse_candidates: FrozenMap<VersionSetId, Vec<SolvableId>>,

    /// A mapping of `VersionSetId` to a sorted list of candidates that match that set.
    pub(crate) version_set_to_sorted_candidates: FrozenMap<VersionSetId, Vec<SolvableId>>,

    /// A mapping from a solvable to a list of dependencies
    solvable_dependencies: Arena<DependenciesId, Dependencies>,
    solvable_to_dependencies: FrozenCopyMap<SolvableId, DependenciesId>,

    /// A mapping that indicates that the dependencies for a particular solvable can cheaply be
    /// retrieved from the dependency provider. This information is provided by the
    /// DependencyProvider when the candidates for a package are requested.
    hint_dependencies_available: RefCell<BitVec>,

    _data: PhantomData<(VS, N)>,
}

impl<VS: VersionSet, N: PackageName, D: DependencyProvider<VS, N>> SolverCache<VS, N, D> {
    /// Constructs a new instance from a provider.
    pub fn new(provider: D) -> Self {
        Self {
            provider,

            candidates: Default::default(),
            package_name_to_candidates: Default::default(),
            version_set_candidates: Default::default(),
            version_set_inverse_candidates: Default::default(),
            version_set_to_sorted_candidates: Default::default(),
            solvable_dependencies: Default::default(),
            solvable_to_dependencies: Default::default(),
            hint_dependencies_available: Default::default(),

            _data: Default::default(),
        }
    }

    /// Returns a reference to the pool used by the solver
    pub fn pool(&self) -> &Pool<VS, N> {
        self.provider.pool()
    }

    /// Returns the candidates for the package with the given name. This will either ask the
    /// [`DependencyProvider`] for the entries or a cached value.
    pub fn get_or_cache_candidates(&self, package_name: NameId) -> &Candidates {
        // If we already have the candidates for this package cached we can simply return
        let candidates_id = match self.package_name_to_candidates.get_copy(&package_name) {
            Some(id) => id,
            None => {
                // Otherwise we have to get them from the DependencyProvider
                let candidates = self
                    .provider
                    .get_candidates(package_name)
                    .unwrap_or_default();

                // Store information about which solvables dependency information is easy to
                // retrieve.
                {
                    let mut hint_dependencies_available =
                        self.hint_dependencies_available.borrow_mut();
                    for hint_candidate in candidates.hint_dependencies_available.iter() {
                        let idx = hint_candidate.to_usize();
                        if hint_dependencies_available.len() <= idx {
                            hint_dependencies_available.resize(idx + 1, false);
                        }
                        hint_dependencies_available.set(idx, true)
                    }
                }

                // Allocate an ID so we can refer to the candidates from everywhere
                let candidates_id = self.candidates.alloc(candidates);
                self.package_name_to_candidates
                    .insert_copy(package_name, candidates_id);

                candidates_id
            }
        };

        // Returns a reference from the arena
        &self.candidates[candidates_id]
    }

    /// Returns the candidates of a package that match the specified version set.
    pub fn get_or_cache_matching_candidates(&self, version_set_id: VersionSetId) -> &[SolvableId] {
        match self.version_set_candidates.get(&version_set_id) {
            Some(candidates) => candidates,
            None => {
                let package_name = self.pool().resolve_version_set_package_name(version_set_id);
                let version_set = self.pool().resolve_version_set(version_set_id);
                let candidates = self.get_or_cache_candidates(package_name);

                let matching_candidates = candidates
                    .candidates
                    .iter()
                    .copied()
                    .filter(|&p| {
                        let version = self.pool().resolve_internal_solvable(p).solvable().inner();
                        version_set.contains(version)
                    })
                    .collect();

                self.version_set_candidates
                    .insert(version_set_id, matching_candidates)
            }
        }
    }

    /// Returns the candidates that do *not* match the specified requirement.
    pub fn get_or_cache_non_matching_candidates(
        &self,
        version_set_id: VersionSetId,
    ) -> &[SolvableId] {
        match self.version_set_inverse_candidates.get(&version_set_id) {
            Some(candidates) => candidates,
            None => {
                let package_name = self.pool().resolve_version_set_package_name(version_set_id);
                let version_set = self.pool().resolve_version_set(version_set_id);
                let candidates = self.get_or_cache_candidates(package_name);

                let matching_candidates = candidates
                    .candidates
                    .iter()
                    .copied()
                    .filter(|&p| {
                        let version = self.pool().resolve_internal_solvable(p).solvable().inner();
                        !version_set.contains(version)
                    })
                    .collect();

                self.version_set_inverse_candidates
                    .insert(version_set_id, matching_candidates)
            }
        }
    }

    /// Returns the candidates for the package with the given name similar to
    /// [`Self::get_or_cache_candidates`] sorted from highest to lowest.
    pub fn get_or_cache_sorted_candidates(&self, version_set_id: VersionSetId) -> &[SolvableId] {
        match self.version_set_to_sorted_candidates.get(&version_set_id) {
            Some(canidates) => canidates,
            None => {
                let package_name = self.pool().resolve_version_set_package_name(version_set_id);
                let matching_candidates = self.get_or_cache_matching_candidates(version_set_id);
                let candidates = self.get_or_cache_candidates(package_name);

                // Sort all the candidates in order in which they should betried by the solver.
                let mut sorted_candidates = Vec::new();
                sorted_candidates.extend_from_slice(matching_candidates);
                self.provider.sort_candidates(self, &mut sorted_candidates);

                // If we have a solvable that we favor, we sort that to the front. This ensures
                // that the version that is favored is picked first.
                if let Some(favored_id) = candidates.favored {
                    if let Some(pos) = sorted_candidates.iter().position(|&s| s == favored_id) {
                        // Move the element at `pos` to the front of the array
                        sorted_candidates[0..=pos].rotate_right(1);
                    }
                }

                self.version_set_to_sorted_candidates
                    .insert(version_set_id, sorted_candidates)
            }
        }
    }

    /// Returns the dependencies of a solvable. Requests the solvables from the
    /// [`DependencyProvider`] if they are not known yet.
    pub fn get_or_cache_dependencies(&self, solvable_id: SolvableId) -> &Dependencies {
        let dependencies_id = match self.solvable_to_dependencies.get_copy(&solvable_id) {
            Some(id) => id,
            None => {
                let dependencies = self.provider.get_dependencies(solvable_id);
                let dependencies_id = self.solvable_dependencies.alloc(dependencies);
                self.solvable_to_dependencies
                    .insert_copy(solvable_id, dependencies_id);
                dependencies_id
            }
        };

        &self.solvable_dependencies[dependencies_id]
    }

    /// Returns true if the dependencies for the given solvable are "cheaply" available. This means
    /// either the dependency provider indicated that the dependencies for a solvable are available
    /// or the dependencies have already been requested.
    pub fn are_dependencies_available_for(&self, solvable: SolvableId) -> bool {
        if self.solvable_to_dependencies.get_copy(&solvable).is_some() {
            true
        } else {
            let solvable_idx = solvable.to_usize();
            let hint_dependencies_available = self.hint_dependencies_available.borrow();
            let value = hint_dependencies_available
                .get(solvable_idx)
                .as_deref()
                .copied();
            value.unwrap_or(false)
        }
    }
}
