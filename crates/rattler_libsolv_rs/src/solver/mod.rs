use crate::{
    internal::{
        arena::{Arena, ArenaId},
        frozen_copy_map::FrozenCopyMap,
        id::{CandidatesId, ClauseId, DependenciesId, LearntClauseId, NameId, SolvableId},
        mapping::Mapping,
    },
    pool::Pool,
    problem::Problem,
    solvable::SolvableInner,
    Candidates, Dependencies, DependencyProvider, PackageName, VersionSet, VersionSetId,
};

use elsa::{FrozenMap, FrozenVec};
use std::{collections::HashSet, fmt::Display, marker::PhantomData};

use clause::{Clause, ClauseState, Literal};
use decision::Decision;
use decision_tracker::DecisionTracker;
use watch_map::WatchMap;

pub(crate) mod clause;
mod decision;
mod decision_map;
mod decision_tracker;
mod watch_map;

/// Drives the SAT solving process
pub struct Solver<VS: VersionSet, N: PackageName, D: DependencyProvider<VS, N>> {
    provider: D,

    pub(crate) clauses: Arena<ClauseId, ClauseState>,
    watches: WatchMap,

    learnt_clauses: Arena<LearntClauseId, Vec<Literal>>,
    learnt_why: Mapping<LearntClauseId, Vec<ClauseId>>,
    learnt_clause_ids: Vec<ClauseId>,

    /// A mapping from a solvable to a list of dependencies
    solvable_dependencies: Arena<DependenciesId, Dependencies>,
    solvable_to_dependencies: FrozenCopyMap<SolvableId, DependenciesId>,

    /// A mapping from package name to a list of candidates.
    candidates: Arena<CandidatesId, Candidates>,
    package_name_to_candidates: FrozenCopyMap<NameId, CandidatesId>,

    // HACK: Since we are not allowed to iterate over the `package_name_to_candidates` field, we
    // store the keys in a seperate vec.
    package_names: FrozenVec<NameId>,

    /// A mapping of `VersionSetId` to the candidates that match that set.
    version_set_candidates: FrozenMap<VersionSetId, Vec<SolvableId>>,

    /// A mapping of `VersionSetId` to the candidates that do not match that set.
    version_set_inverse_candidates: FrozenMap<VersionSetId, Vec<SolvableId>>,

    /// A mapping of `VersionSetId` to a sorted list of canidates that match that set.
    version_set_to_sorted_candidates: FrozenMap<VersionSetId, Vec<SolvableId>>,

    decision_tracker: DecisionTracker,

    /// The version sets that must be installed as part of the solution.
    root_requirements: Vec<VersionSetId>,

    _data: PhantomData<(VS, N)>,
}

impl<VS: VersionSet, N: PackageName, D: DependencyProvider<VS, N>> Solver<VS, N, D> {
    /// Create a solver, using the provided pool
    pub fn new(provider: D) -> Self {
        Self {
            provider,
            clauses: Arena::new(),
            watches: WatchMap::new(),
            learnt_clauses: Arena::new(),
            learnt_why: Mapping::new(),
            learnt_clause_ids: Vec::new(),
            decision_tracker: DecisionTracker::new(),
            candidates: Arena::new(),
            solvable_dependencies: Default::default(),
            solvable_to_dependencies: Default::default(),
            package_name_to_candidates: Default::default(),
            package_names: Default::default(),
            root_requirements: Default::default(),
            version_set_candidates: Default::default(),
            version_set_inverse_candidates: Default::default(),
            version_set_to_sorted_candidates: Default::default(),
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

                // Allocate an ID so we can refer to the candidates from everywhere
                let candidates_id = self.candidates.alloc(candidates);
                self.package_name_to_candidates
                    .insert_copy(package_name, candidates_id);
                self.package_names.push(package_name);

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
}

impl<VS: VersionSet, N: PackageName + Display, D: DependencyProvider<VS, N>> Solver<VS, N, D> {
    /// Solves the provided `jobs` and returns a transaction from the found solution
    ///
    /// Returns a [`Problem`] if no solution was found, which provides ways to inspect the causes
    /// and report them to the user.
    pub fn solve(
        &mut self,
        root_requirements: Vec<VersionSetId>,
    ) -> Result<Vec<SolvableId>, Problem> {
        // Clear state
        self.decision_tracker.clear();
        self.learnt_clauses.clear();
        self.learnt_why = Mapping::new();
        self.clauses = Default::default();
        self.root_requirements = root_requirements;

        // The first clause will always be the install root clause. Here we verify that this is
        // indeed the case.
        let root_clause = self.clauses.alloc(ClauseState::root());
        assert_eq!(root_clause, ClauseId::install_root());

        // Create clauses for root's dependencies, and their dependencies, and so forth
        self.add_clauses_for_root_deps();

        // Add clauses ensuring only a single candidate per package name is installed
        // TODO: (BasZ) Im pretty sure there is a better way to formulate this. Maybe take a look
        //   at pubgrub?
        // TODO: Can we move this to where a package is added?
        for package_name in
            (0..self.package_names.len()).map(|idx| self.package_names.get_copy(idx).unwrap())
        {
            let candidates_id = self
                .package_name_to_candidates
                .get_copy(&package_name)
                .unwrap();
            let candidates = &self.candidates[candidates_id].candidates;
            // Each candidate gets a clause with each other candidate
            for (i, &candidate) in candidates.iter().enumerate() {
                for &other_candidate in &candidates[i + 1..] {
                    self.clauses
                        .alloc(ClauseState::forbid_multiple(candidate, other_candidate));
                }
            }
        }

        // Add clauses for the locked solvable
        // TODO: Can we somehow move this to where a package is added?
        for package_name in
            (0..self.package_names.len()).map(|idx| self.package_names.get_copy(idx).unwrap())
        {
            let candidates_id = self
                .package_name_to_candidates
                .get_copy(&package_name)
                .unwrap();
            let candidates = &self.candidates[candidates_id];
            let Some(locked_solvable_id) = candidates.locked else { continue };
            // For each locked solvable, forbid other solvables with the same name
            for &other_candidate in &candidates.candidates {
                if other_candidate != locked_solvable_id {
                    self.clauses
                        .alloc(ClauseState::lock(locked_solvable_id, other_candidate));
                }
            }
        }

        // Create watches chains
        self.make_watches();

        // Run SAT
        self.run_sat()?;

        let steps = self
            .decision_tracker
            .stack()
            .iter()
            .flat_map(|d| {
                if d.value && d.solvable_id != SolvableId::root() {
                    Some(d.solvable_id)
                } else {
                    // Ignore things that are set to false
                    None
                }
            })
            .collect();

        Ok(steps)
    }

    /// Adds clauses for root's dependencies, their dependencies, and so forth
    ///
    /// This function makes sure we only generate clauses for the solvables involved in the problem,
    /// traversing the graph of requirements and ignoring unrelated packages. The graph is
    /// traversed depth-first.
    ///
    /// A side effect of this function is that candidates for all involved match specs (in the
    /// dependencies or constrains part of the package record) are fetched and cached for future
    /// use. The `favored_map` parameter influences the order in which the candidates for a
    /// dependency are sorted, giving preference to the favored package (i.e. placing it at the
    /// front).
    fn add_clauses_for_root_deps(&mut self) {
        let mut visited = HashSet::new();
        let mut stack = Vec::new();

        stack.push(SolvableId::root());

        let mut seen_requires = HashSet::new();
        let empty_version_set_id_vec: Vec<VersionSetId> = Vec::new();

        while let Some(solvable_id) = stack.pop() {
            let solvable = self.pool().resolve_internal_solvable(solvable_id);

            // Determine the dependencies of the current solvable. There are two cases here:
            // 1. The solvable is the root solvable which only provides required dependencies.
            // 2. The solvable is a package candidate in which case we request the corresponding
            //    dependencies from the `DependencyProvider`.
            let (requirements, constrains) = match solvable.inner {
                SolvableInner::Root => (&self.root_requirements, &empty_version_set_id_vec),
                SolvableInner::Package(_) => {
                    let deps = self.get_or_cache_dependencies(solvable_id);
                    (&deps.requirements, &deps.constrains)
                }
            };

            // Iterate over all the requirements and create clauses.
            for &version_set_id in requirements {
                // Get the sorted candidates that can fulfill this requirement
                let candidates = self.get_or_cache_sorted_candidates(version_set_id);

                // Add any of the candidates to the stack of solvables that we need to visit. Only
                // do this if we didnt visit the requirement before. Multiple solvables can share
                // the same [`VersionSetId`] if they specify the exact same requirement.
                if seen_requires.insert(version_set_id) {
                    for &candidate in candidates.iter() {
                        if visited.insert(candidate) {
                            stack.push(candidate);
                        }
                    }
                }

                self.clauses.alloc(ClauseState::requires(
                    solvable_id,
                    version_set_id,
                    candidates,
                ));
            }

            // Iterate over all constrains and add clauses
            for &version_set_id in constrains.as_slice() {
                // Get the candidates that do-not match the specified requirement.
                let non_candidates = self
                    .get_or_cache_non_matching_candidates(version_set_id)
                    .iter()
                    .copied();

                // Add forbidden clauses for the candidates
                for forbidden_candidate in non_candidates {
                    let clause =
                        ClauseState::constrains(solvable_id, forbidden_candidate, version_set_id);
                    self.clauses.alloc(clause);
                }
            }
        }
    }

    /// Run the CDCL algorithm to solve the SAT problem
    ///
    /// The CDCL algorithm's job is to find a valid assignment to the variables involved in the
    /// provided clauses. It works in the following steps:
    ///
    /// 1. __Set__: Assign a value to a variable that hasn't been assigned yet. An assignment in
    ///    this step starts a new "level" (the first one being level 1). If all variables have been
    ///    assigned, then we are done.
    /// 2. __Propagate__: Perform [unit
    ///    propagation](https://en.wikipedia.org/wiki/Unit_propagation). Assignments in this step
    ///    are associated to the same "level" as the decision that triggered them. This "level"
    ///    metadata is useful when it comes to handling conflicts. See [`Solver::propagate`] for the
    ///    implementation of this step.
    /// 3. __Learn__: If propagation finishes without conflicts, go back to 1. Otherwise find the
    ///    combination of assignments that caused the conflict and add a new clause to the solver to
    ///    forbid that combination of assignments (i.e. learn from this mistake so it is not
    ///    repeated in the future). Then backtrack and go back to step 1 or, if the learnt clause is
    ///    in conflict with existing clauses, declare the problem to be unsolvable. See
    ///    [`Solver::analyze`] for the implementation of this step.
    ///
    /// The solver loop can be found in [`Solver::resolve_dependencies`].
    fn run_sat(&mut self) -> Result<(), Problem> {
        assert!(self.decision_tracker.is_empty());

        // Assign `true` to the root solvable
        let level = 1;
        self.decision_tracker
            .try_add_decision(
                Decision::new(SolvableId::root(), true, ClauseId::install_root()),
                1,
            )
            .expect("bug: solvable was already decided!");

        // Forbid packages that rely on dependencies without candidates
        self.decide_requires_without_candidates(level)
            .map_err(|cause| self.analyze_unsolvable(cause))?;

        // Propagate after the assignments above
        self.propagate(level)
            .map_err(|(_, _, cause)| self.analyze_unsolvable(cause))?;

        // Enter the solver loop
        self.resolve_dependencies(level)?;

        Ok(())
    }

    /// Forbid packages that rely on dependencies without candidates
    ///
    /// Since a requires clause is represented as (¬A ∨ candidate_1 ∨ ... ∨ candidate_n),
    /// a dependency without candidates becomes (¬A), which means that A should always be false.
    fn decide_requires_without_candidates(&mut self, level: u32) -> Result<(), ClauseId> {
        tracing::info!("=== Deciding assertions for requires without candidates");

        for (clause_id, clause) in self.clauses.iter() {
            if let Clause::Requires(solvable_id, _) = clause.kind {
                if !clause.has_watches() {
                    // A requires clause without watches means it has a single literal (i.e.
                    // there are no candidates)
                    let decided = self
                        .decision_tracker
                        .try_add_decision(Decision::new(solvable_id, false, clause_id), level)
                        .map_err(|_| clause_id)?;

                    if decided {
                        tracing::info!("Set {} = false", solvable_id.display(self.pool()));
                    }
                }
            }
        }

        Ok(())
    }

    /// Resolves all dependencies
    ///
    /// Repeatedly chooses the next variable to assign, and calls [`Solver::set_propagate_learn`] to
    /// drive the solving process (as you can see from the name, the method executes the set,
    /// propagate and learn steps described in the [`Solver::run_sat`] docs).
    ///
    /// The next variable to assign is obtained by finding the next dependency for which no concrete
    /// package has been picked yet. Then we pick the highest possible version for that package, or
    /// the favored version if it was provided by the user, and set its value to true.
    fn resolve_dependencies(&mut self, mut level: u32) -> Result<u32, Problem> {
        let mut i = 0;
        loop {
            if i >= self.clauses.len() {
                break;
            }

            let clause_id = ClauseId::from_usize(i);

            let (required_by, candidate) = {
                let clause = &self.clauses[clause_id];
                i += 1;

                // We are only interested in requires clauses
                let Clause::Requires(solvable_id, deps) = clause.kind else {
                    continue;
                };

                // Consider only clauses in which we have decided to install the solvable
                if self.decision_tracker.assigned_value(solvable_id) != Some(true) {
                    continue;
                }

                // Consider only clauses in which no candidates have been installed
                let candidates = &self.version_set_to_sorted_candidates[&deps];
                if candidates
                    .iter()
                    .any(|&c| self.decision_tracker.assigned_value(c) == Some(true))
                {
                    continue;
                }

                // Get the first candidate that is undecided and should be installed
                //
                // This assumes that the packages have been provided in the right order when the solvables were created
                // (most recent packages first)
                (
                    solvable_id,
                    candidates
                        .iter()
                        .cloned()
                        .find(|&c| self.decision_tracker.assigned_value(c).is_none())
                        .unwrap(),
                )
            };

            level = self.set_propagate_learn(level, candidate, required_by, clause_id)?;

            // We have made progress, and should look at all clauses in the next iteration
            i = 0;
        }

        // We just went through all clauses and there are no choices left to be made
        Ok(level)
    }

    /// Executes one iteration of the CDCL loop
    ///
    /// A set-propagate-learn round is always initiated by a requirement clause (i.e.
    /// [`Clause::Requires`]). The parameters include the variable associated to the candidate for the
    /// dependency (`solvable`), the package that originates the dependency (`required_by`), and the
    /// id of the requires clause (`clause_id`).
    ///
    /// Refer to the documentation of [`Solver::run_sat`] for details on the CDCL algorithm.
    ///
    /// Returns the new level after this set-propagate-learn round, or a [`Problem`] if we
    /// discovered that the requested jobs are unsatisfiable.
    fn set_propagate_learn(
        &mut self,
        mut level: u32,
        solvable: SolvableId,
        required_by: SolvableId,
        clause_id: ClauseId,
    ) -> Result<u32, Problem> {
        level += 1;

        tracing::info!(
            "╤══ Install {} at level {level} (required by {})",
            solvable.display(self.pool()),
            required_by.display(self.pool()),
        );

        self.decision_tracker
            .try_add_decision(Decision::new(solvable, true, clause_id), level)
            .expect("bug: solvable was already decided!");

        loop {
            let r = self.propagate(level);
            let Err((conflicting_solvable, attempted_value, conflicting_clause)) = r else {
                // Propagation succeeded
                tracing::info!("╘══ Propagation succeeded");
                break;
            };

            {
                tracing::info!(
                    "├─ Propagation conflicted: could not set {solvable} to {attempted_value}",
                    solvable = solvable.display(self.pool())
                );
                tracing::info!(
                    "│  During unit propagation for clause: {:?}",
                    self.clauses[conflicting_clause].debug(self.pool())
                );

                tracing::info!(
                    "│  Previously decided value: {}. Derived from: {:?}",
                    !attempted_value,
                    self.clauses[self
                        .decision_tracker
                        .stack()
                        .iter()
                        .find(|d| d.solvable_id == conflicting_solvable)
                        .unwrap()
                        .derived_from]
                        .debug(self.pool()),
                );
            }

            if level == 1 {
                tracing::info!("╘══ UNSOLVABLE");
                for decision in self.decision_tracker.stack() {
                    let clause = &self.clauses[decision.derived_from];
                    let level = self.decision_tracker.level(decision.solvable_id);
                    let action = if decision.value { "install" } else { "forbid" };

                    if let Clause::ForbidMultipleInstances(..) = clause.kind {
                        // Skip forbids clauses, to reduce noise
                        continue;
                    }

                    tracing::info!(
                        "* ({level}) {action} {}. Reason: {:?}",
                        decision.solvable_id.display(self.pool()),
                        clause.debug(self.pool()),
                    );
                }

                return Err(self.analyze_unsolvable(conflicting_clause));
            }

            let (new_level, learned_clause_id, literal) =
                self.analyze(level, conflicting_solvable, conflicting_clause);
            level = new_level;

            tracing::info!("├─ Backtracked to level {level}");

            // Optimization: propagate right now, since we know that the clause is a unit clause
            let decision = literal.satisfying_value();
            self.decision_tracker
                .try_add_decision(
                    Decision::new(literal.solvable_id, decision, learned_clause_id),
                    level,
                )
                .expect("bug: solvable was already decided!");
            tracing::info!(
                "├─ Propagate after learn: {} = {decision}",
                literal.solvable_id.display(self.pool())
            );
        }

        Ok(level)
    }

    /// The propagate step of the CDCL algorithm
    ///
    /// Propagation is implemented by means of watches: each clause that has two or more literals is
    /// "subscribed" to changes in the values of two solvables that appear in the clause. When a value
    /// is assigned to a solvable, each of the clauses tracking that solvable will be notified. That
    /// way, the clause can check whether the literal that is using the solvable has become false, in
    /// which case it picks a new solvable to watch (if available) or triggers an assignment.
    fn propagate(&mut self, level: u32) -> Result<(), (SolvableId, bool, ClauseId)> {
        // Learnt assertions (assertions are clauses that consist of a single literal, and therefore
        // do not have watches)
        for &clause_id in self.learnt_clause_ids.iter() {
            let clause = &self.clauses[clause_id];
            let Clause::Learnt(learnt_index) = clause.kind else {
                unreachable!();
            };

            let literals = &self.learnt_clauses[learnt_index];
            if literals.len() > 1 {
                continue;
            }

            debug_assert!(!literals.is_empty());

            let literal = literals[0];
            let decision = literal.satisfying_value();

            let decided = self
                .decision_tracker
                .try_add_decision(
                    Decision::new(literal.solvable_id, decision, clause_id),
                    level,
                )
                .map_err(|_| (literal.solvable_id, decision, clause_id))?;

            if decided {
                tracing::info!(
                    "├─ Propagate assertion {} = {}",
                    literal.solvable_id.display(self.pool()),
                    decision
                );
            }
        }

        // Watched solvables
        while let Some(decision) = self.decision_tracker.next_unpropagated() {
            let pkg = decision.solvable_id;

            // Propagate, iterating through the linked list of clauses that watch this solvable
            let mut old_predecessor_clause_id: Option<ClauseId>;
            let mut predecessor_clause_id: Option<ClauseId> = None;
            let mut clause_id = self.watches.first_clause_watching_solvable(pkg);
            while !clause_id.is_null() {
                if predecessor_clause_id == Some(clause_id) {
                    panic!("Linked list is circular!");
                }

                // Get mutable access to both clauses.
                let (predecessor_clause, clause) =
                    if let Some(prev_clause_id) = predecessor_clause_id {
                        let (predecessor_clause, clause) =
                            self.clauses.get_two_mut(prev_clause_id, clause_id);
                        (Some(predecessor_clause), clause)
                    } else {
                        (None, &mut self.clauses[clause_id])
                    };

                // Update the prev_clause_id for the next run
                old_predecessor_clause_id = predecessor_clause_id;
                predecessor_clause_id = Some(clause_id);

                // Configure the next clause to visit
                let this_clause_id = clause_id;
                clause_id = clause.next_watched_clause(pkg);

                if let Some((watched_literals, watch_index)) = clause.watch_turned_false(
                    pkg,
                    self.decision_tracker.map(),
                    &self.learnt_clauses,
                ) {
                    // One of the watched literals is now false
                    if let Some(variable) = clause.next_unwatched_variable(
                        &self.learnt_clauses,
                        &self.version_set_to_sorted_candidates,
                        self.decision_tracker.map(),
                    ) {
                        debug_assert!(!clause.watched_literals.contains(&variable));

                        self.watches.update_watched(
                            predecessor_clause,
                            clause,
                            this_clause_id,
                            watch_index,
                            pkg,
                            variable,
                        );

                        // Make sure the right predecessor is kept for the next iteration (i.e. the
                        // current clause is no longer a predecessor of the next one; the current
                        // clause's predecessor is)
                        predecessor_clause_id = old_predecessor_clause_id;
                    } else {
                        // We could not find another literal to watch, which means the remaining
                        // watched literal can be set to true
                        let remaining_watch_index = match watch_index {
                            0 => 1,
                            1 => 0,
                            _ => unreachable!(),
                        };

                        let remaining_watch = watched_literals[remaining_watch_index];
                        let decided = self
                            .decision_tracker
                            .try_add_decision(
                                Decision::new(
                                    remaining_watch.solvable_id,
                                    remaining_watch.satisfying_value(),
                                    this_clause_id,
                                ),
                                level,
                            )
                            .map_err(|_| (remaining_watch.solvable_id, true, this_clause_id))?;

                        if decided {
                            match clause.kind {
                                // Skip logging for ForbidMultipleInstances, which is so noisy
                                Clause::ForbidMultipleInstances(..) => {}
                                _ => {
                                    tracing::info!(
                                        "├─ Propagate {} = {}. {:?}",
                                        remaining_watch.solvable_id.display(self.provider.pool()),
                                        remaining_watch.satisfying_value(),
                                        clause.debug(self.provider.pool()),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Adds the clause with `clause_id` to the current `Problem`
    ///
    /// Because learnt clauses are not relevant for the user, they are not added to the `Problem`.
    /// Instead, we report the clauses that caused them.
    fn analyze_unsolvable_clause(
        clauses: &Arena<ClauseId, ClauseState>,
        learnt_why: &Mapping<LearntClauseId, Vec<ClauseId>>,
        clause_id: ClauseId,
        problem: &mut Problem,
        seen: &mut HashSet<ClauseId>,
    ) {
        let clause = &clauses[clause_id];
        match clause.kind {
            Clause::Learnt(learnt_clause_id) => {
                if !seen.insert(clause_id) {
                    return;
                }

                for &cause in learnt_why
                    .get(learnt_clause_id)
                    .expect("no cause for learnt clause available")
                {
                    Self::analyze_unsolvable_clause(clauses, learnt_why, cause, problem, seen);
                }
            }
            _ => problem.add_clause(clause_id),
        }
    }

    /// Create a [`Problem`] based on the id of the clause that triggered an unrecoverable conflict
    fn analyze_unsolvable(&mut self, clause_id: ClauseId) -> Problem {
        let last_decision = self.decision_tracker.stack().last().unwrap();
        let highest_level = self.decision_tracker.level(last_decision.solvable_id);
        debug_assert_eq!(highest_level, 1);

        let mut problem = Problem::default();

        tracing::info!("=== ANALYZE UNSOLVABLE");

        let mut involved = HashSet::new();
        self.clauses[clause_id].kind.visit_literals(
            &self.learnt_clauses,
            &self.version_set_to_sorted_candidates,
            |literal| {
                involved.insert(literal.solvable_id);
            },
        );

        let mut seen = HashSet::new();
        Self::analyze_unsolvable_clause(
            &self.clauses,
            &self.learnt_why,
            clause_id,
            &mut problem,
            &mut seen,
        );

        for decision in self.decision_tracker.stack()[1..].iter().rev() {
            if decision.solvable_id == SolvableId::root() {
                panic!("unexpected root solvable")
            }

            let why = decision.derived_from;

            if !involved.contains(&decision.solvable_id) {
                continue;
            }

            assert_ne!(why, ClauseId::install_root());

            Self::analyze_unsolvable_clause(
                &self.clauses,
                &self.learnt_why,
                why,
                &mut problem,
                &mut seen,
            );

            self.clauses[why].kind.visit_literals(
                &self.learnt_clauses,
                &self.version_set_to_sorted_candidates,
                |literal| {
                    if literal.eval(self.decision_tracker.map()) == Some(true) {
                        assert_eq!(literal.solvable_id, decision.solvable_id);
                    } else {
                        involved.insert(literal.solvable_id);
                    }
                },
            );
        }

        problem
    }

    /// Analyze the causes of the conflict and learn from it
    ///
    /// This function finds the combination of assignments that caused the conflict and adds a new
    /// clause to the solver to forbid that combination of assignments (i.e. learn from this mistake
    /// so it is not repeated in the future). It corresponds to the `Solver.analyze` function from
    /// the MiniSAT paper.
    ///
    /// Returns the level to which we should backtrack, the id of the learnt clause and the literal
    /// that should be assigned (by definition, when we learn a clause, all its literals except one
    /// evaluate to false, so the value of the remaining literal must be assigned to make the clause
    /// become true)
    fn analyze(
        &mut self,
        mut current_level: u32,
        mut conflicting_solvable: SolvableId,
        mut clause_id: ClauseId,
    ) -> (u32, ClauseId, Literal) {
        let mut seen = HashSet::new();
        let mut causes_at_current_level = 0u32;
        let mut learnt = Vec::new();
        let mut back_track_to = 0;

        let mut s_value;
        let mut learnt_why = Vec::new();
        let mut first_iteration = true;
        loop {
            learnt_why.push(clause_id);

            self.clauses[clause_id].kind.visit_literals(
                &self.learnt_clauses,
                &self.version_set_to_sorted_candidates,
                |literal| {
                    if !first_iteration && literal.solvable_id == conflicting_solvable {
                        // We are only interested in the causes of the conflict, so we ignore the
                        // solvable whose value was propagated
                        return;
                    }

                    if !seen.insert(literal.solvable_id) {
                        // Skip literals we have already seen
                        return;
                    }

                    let decision_level = self.decision_tracker.level(literal.solvable_id);
                    if decision_level == current_level {
                        causes_at_current_level += 1;
                    } else if current_level > 1 {
                        let learnt_literal = Literal {
                            solvable_id: literal.solvable_id,
                            negate: self
                                .decision_tracker
                                .assigned_value(literal.solvable_id)
                                .unwrap(),
                        };
                        learnt.push(learnt_literal);
                        back_track_to = back_track_to.max(decision_level);
                    } else {
                        unreachable!();
                    }
                },
            );

            first_iteration = false;

            // Select next literal to look at
            loop {
                let (last_decision, last_decision_level) = self.decision_tracker.undo_last();

                conflicting_solvable = last_decision.solvable_id;
                s_value = last_decision.value;
                clause_id = last_decision.derived_from;

                current_level = last_decision_level;

                // We are interested in the first literal we come across that caused the conflicting
                // assignment
                if seen.contains(&last_decision.solvable_id) {
                    break;
                }
            }

            causes_at_current_level = causes_at_current_level.saturating_sub(1);
            if causes_at_current_level == 0 {
                break;
            }
        }

        let last_literal = Literal {
            solvable_id: conflicting_solvable,
            negate: s_value,
        };
        learnt.push(last_literal);

        // Add the clause
        let learnt_id = self.learnt_clauses.alloc(learnt.clone());
        self.learnt_why.insert(learnt_id, learnt_why);

        let clause_id = self.clauses.alloc(ClauseState::learnt(learnt_id, &learnt));
        self.learnt_clause_ids.push(clause_id);

        let clause = &mut self.clauses[clause_id];
        if clause.has_watches() {
            self.watches.start_watching(clause, clause_id);
        }

        tracing::info!("├─ Learnt disjunction:",);
        for lit in learnt {
            tracing::info!(
                "│  - {}{}",
                if lit.negate { "NOT " } else { "" },
                lit.solvable_id.display(self.pool())
            );
        }

        // Should revert at most to the root level
        let target_level = back_track_to.max(1);
        self.decision_tracker.undo_until(target_level);

        (target_level, clause_id, last_literal)
    }

    fn make_watches(&mut self) {
        // Watches are already initialized in the clauses themselves, here we build a linked list for
        // each package (a clause will be linked to other clauses that are watching the same package)
        for (clause_id, clause) in self.clauses.iter_mut() {
            if !clause.has_watches() {
                // Skip clauses without watches
                continue;
            }

            self.watches.start_watching(clause, clause_id);
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{range::Range, Candidates, DefaultSolvableDisplay, Dependencies};
    use indexmap::IndexMap;
    use std::num::ParseIntError;
    use std::{
        collections::HashMap,
        fmt::{Debug, Display, Formatter},
        str::FromStr,
    };

    // Let's define our own packaging version system and dependency specification.
    // This is a very simple version system, where a package is identified by a name and a version
    // in which the version is just an integer. The version is a range so can be noted as 0..2
    // or something of the sorts, we also support constrains which means it should not use that
    // package version this is also represented with a range.
    //
    // You can also use just a single number for a range like `package 0` which means the range from 0..1 (excluding the end)
    //
    // Lets call the tuples of (Name, Version) a `Pack` and the tuples of (Name, Range<u32>) a `Spec`
    //
    // We also need to create a custom provider that tells us how to sort the candidates. This is unique to each
    // packaging ecosystem. Let's call our ecosystem 'BundleBox' so that how we call the provider as well.

    /// This is `Pack` which is a unique version and name in our bespoke packaging system
    #[derive(Debug, Ord, PartialOrd, Eq, PartialEq, Copy, Clone, Hash)]
    #[repr(transparent)]
    struct Pack(u32);

    impl From<u32> for Pack {
        fn from(value: u32) -> Self {
            Pack(value)
        }
    }

    impl From<i32> for Pack {
        fn from(value: i32) -> Self {
            Pack(value as u32)
        }
    }

    impl Display for Pack {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl FromStr for Pack {
        type Err = ParseIntError;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            u32::from_str(s).map(Self)
        }
    }

    /// We can use this to see if a `Pack` is contained in a range of package versions or a `Spec`
    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct Spec {
        name: String,
        versions: Range<Pack>,
    }

    impl Spec {
        pub fn new(name: String, versions: Range<Pack>) -> Self {
            Self { name, versions }
        }
    }

    impl FromStr for Spec {
        type Err = ();

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            let split = s.split(' ').collect::<Vec<_>>();
            let name = split
                .first()
                .expect("spec does not have a name")
                .to_string();

            fn version_range(s: Option<&&str>) -> Range<Pack> {
                if let Some(s) = s {
                    let (start, end) = s
                        .split_once("..")
                        .map_or((*s, None), |(start, end)| (start, Some(end)));
                    let start: Pack = start.parse().unwrap();
                    let end = end
                        .map(FromStr::from_str)
                        .transpose()
                        .unwrap()
                        .unwrap_or(Pack(start.0 + 1));
                    Range::between(start, end)
                } else {
                    Range::full()
                }
            }

            let versions = version_range(split.get(1));

            Ok(Spec::new(name, versions))
        }
    }

    /// This provides sorting functionality for our `BundleBox` packaging system
    #[derive(Default)]
    struct BundleBoxProvider {
        pool: Pool<Range<Pack>>,
        packages: IndexMap<String, IndexMap<Pack, BundleBoxPackageDependencies>>,
        favored: HashMap<String, Pack>,
        locked: HashMap<String, Pack>,
    }

    struct BundleBoxPackageDependencies {
        dependencies: Vec<Spec>,
        constrains: Vec<Spec>,
    }

    impl BundleBoxProvider {
        pub fn new() -> Self {
            Default::default()
        }

        pub fn requirements(&self, requirements: &[&str]) -> Vec<VersionSetId> {
            requirements
                .into_iter()
                .map(|dep| Spec::from_str(dep).unwrap())
                .map(|spec| {
                    let dep_name = self.pool.intern_package_name(&spec.name);
                    self.pool
                        .intern_version_set(dep_name, spec.versions.clone())
                })
                .collect()
        }

        pub fn from_packages(packages: &[(&str, u32, Vec<&str>)]) -> Self {
            let mut result = Self::new();
            for (name, version, deps) in packages {
                result.add_package(name, Pack(*version), deps, &[]);
            }
            result
        }

        pub fn set_favored(&mut self, package_name: &str, version: u32) {
            self.favored.insert(package_name.to_owned(), Pack(version));
        }

        pub fn set_locked(&mut self, package_name: &str, version: u32) {
            self.locked.insert(package_name.to_owned(), Pack(version));
        }

        pub fn add_package(
            &mut self,
            package_name: &str,
            package_version: Pack,
            dependencies: &[&str],
            constrains: &[&str],
        ) {
            let dependencies = dependencies
                .into_iter()
                .map(|dep| Spec::from_str(dep))
                .collect::<Result<Vec<_>, _>>()
                .unwrap();

            let constrains = constrains
                .into_iter()
                .map(|dep| Spec::from_str(dep))
                .collect::<Result<Vec<_>, _>>()
                .unwrap();

            self.packages
                .entry(package_name.to_owned())
                .or_default()
                .insert(
                    package_version,
                    BundleBoxPackageDependencies {
                        dependencies,
                        constrains,
                    },
                );
        }
    }

    impl DependencyProvider<Range<Pack>> for BundleBoxProvider {
        fn pool(&self) -> &Pool<Range<Pack>> {
            &self.pool
        }

        fn sort_candidates(
            &self,
            _solver: &Solver<Range<Pack>, String, Self>,
            solvables: &mut [SolvableId],
        ) {
            solvables.sort_by(|a, b| {
                let a = self.pool.resolve_internal_solvable(*a).solvable();
                let b = self.pool.resolve_internal_solvable(*b).solvable();
                // We want to sort with highest version on top
                b.inner.0.cmp(&a.inner.0)
            });
        }

        fn get_candidates(&self, name: NameId) -> Option<Candidates> {
            let package_name = self.pool.resolve_package_name(name);
            let package = self.packages.get(package_name)?;

            let mut candidates = Candidates {
                candidates: Vec::with_capacity(package.len()),
                ..Candidates::default()
            };
            let favor = self.favored.get(package_name);
            let locked = self.locked.get(package_name);
            for pack in package.keys() {
                let solvable = self.pool.intern_solvable(name, *pack);
                candidates.candidates.push(solvable);
                if Some(pack) == favor {
                    candidates.favored = Some(solvable);
                }
                if Some(pack) == locked {
                    candidates.locked = Some(solvable);
                }
            }

            Some(candidates)
        }

        fn get_dependencies(&self, solvable: SolvableId) -> Dependencies {
            let candidate = self.pool.resolve_solvable(solvable);
            let package_name = self.pool.resolve_package_name(candidate.name);
            let pack = candidate.inner();
            let Some(deps) = self.packages.get(package_name).and_then(|v| v.get(pack)) else { return Default::default() };

            let mut result = Dependencies {
                requirements: Vec::with_capacity(deps.dependencies.len()),
                constrains: Vec::with_capacity(deps.constrains.len()),
            };
            for req in &deps.dependencies {
                let dep_name = self.pool.intern_package_name(&req.name);
                let dep_spec = self.pool.intern_version_set(dep_name, req.versions.clone());
                result.requirements.push(dep_spec);
            }

            for req in &deps.constrains {
                let dep_name = self.pool.intern_package_name(&req.name);
                let dep_spec = self.pool.intern_version_set(dep_name, req.versions.clone());
                result.constrains.push(dep_spec);
            }

            result
        }
    }

    /// Create a string from a [`Transaction`]
    fn transaction_to_string<VS: VersionSet>(
        pool: &Pool<VS>,
        solvables: &Vec<SolvableId>,
    ) -> String {
        use std::fmt::Write;
        let mut buf = String::new();
        for &solvable_id in solvables {
            writeln!(buf, "{}", solvable_id.display(pool)).unwrap();
        }

        buf
    }

    /// Unsat so that we can view the problem
    fn solve_unsat(provider: BundleBoxProvider, specs: &[&str]) -> String {
        let requirements = provider.requirements(specs);
        let mut solver = Solver::new(provider);
        match solver.solve(requirements) {
            Ok(_) => panic!("expected unsat, but a solution was found"),
            Err(problem) => problem
                .display_user_friendly(&solver, &DefaultSolvableDisplay)
                .to_string(),
        }
    }

    /// Test whether we can select a version, this is the most basic operation
    #[test]
    fn test_unit_propagation_1() {
        let provider = BundleBoxProvider::from_packages(&[("asdf", 1, vec![])]);
        let root_requirements = provider.requirements(&["asdf"]);
        let mut solver = Solver::new(provider);
        let solved = solver.solve(root_requirements).unwrap();

        assert_eq!(solved.len(), 1);
        let solvable = solver
            .pool()
            .resolve_internal_solvable(solved[0])
            .solvable();

        assert_eq!(solver.pool().resolve_package_name(solvable.name), "asdf");
        assert_eq!(solvable.inner.0, 1);
    }

    /// Test if we can also select a nested version
    #[test]
    fn test_unit_propagation_nested() {
        let provider = BundleBoxProvider::from_packages(&[
            ("asdf", 1u32, vec!["efgh"]),
            ("efgh", 4u32, vec![]),
            ("dummy", 6u32, vec![]),
        ]);
        let requirements = provider.requirements(&["asdf"]);
        let mut solver = Solver::new(provider);
        let solved = solver.solve(requirements).unwrap();

        assert_eq!(solved.len(), 2);

        let solvable = solver
            .pool()
            .resolve_internal_solvable(solved[0])
            .solvable();

        assert_eq!(solver.pool().resolve_package_name(solvable.name), "asdf");
        assert_eq!(solvable.inner.0, 1);

        let solvable = solver
            .pool()
            .resolve_internal_solvable(solved[1])
            .solvable();

        assert_eq!(solver.pool().resolve_package_name(solvable.name), "efgh");
        assert_eq!(solvable.inner.0, 4);
    }

    /// Test if we can resolve multiple versions at once
    #[test]
    fn test_resolve_multiple() {
        let provider = BundleBoxProvider::from_packages(&[
            ("asdf", 1, vec![]),
            ("asdf", 2, vec![]),
            ("efgh", 4, vec![]),
            ("efgh", 5, vec![]),
        ]);
        let requirements = provider.requirements(&["asdf", "efgh"]);
        let mut solver = Solver::new(provider);
        let solved = solver.solve(requirements).unwrap();

        assert_eq!(solved.len(), 2);

        let solvable = solver
            .pool()
            .resolve_internal_solvable(solved[0])
            .solvable();

        assert_eq!(solver.pool().resolve_package_name(solvable.name), "asdf");
        assert_eq!(solvable.inner.0, 2);

        let solvable = solver
            .pool()
            .resolve_internal_solvable(solved[1])
            .solvable();

        assert_eq!(solver.pool().resolve_package_name(solvable.name), "efgh");
        assert_eq!(solvable.inner.0, 5);
    }

    /// In case of a conflict the version should not be selected with the conflict
    #[test]
    fn test_resolve_with_conflict() {
        let provider = BundleBoxProvider::from_packages(&[
            ("asdf", 4, vec!["conflicting 1"]),
            ("asdf", 3, vec!["conflicting 0"]),
            ("efgh", 7, vec!["conflicting 0"]),
            ("efgh", 6, vec!["conflicting 0"]),
            ("conflicting", 1, vec![]),
            ("conflicting", 0, vec![]),
        ]);
        let requirements = provider.requirements(&["asdf", "efgh"]);
        let mut solver = Solver::new(provider);
        let solved = solver.solve(requirements);
        let solved = match solved {
            Ok(solved) => solved,
            Err(p) => panic!(
                "{}",
                p.display_user_friendly(&solver, &DefaultSolvableDisplay)
            ),
        };

        use std::fmt::Write;
        let mut display_result = String::new();
        for &solvable_id in &solved {
            writeln!(
                display_result,
                "{solvable}",
                solvable = solvable_id.display(solver.pool())
            )
            .unwrap();
        }

        insta::assert_snapshot!(display_result);
    }

    /// The non-existing package should not be selected
    #[test]
    fn test_resolve_with_nonexisting() {
        let provider = BundleBoxProvider::from_packages(&[
            ("asdf", 4, vec!["b"]),
            ("asdf", 3, vec![]),
            ("b", 1, vec!["idontexist"]),
        ]);
        let requirements = provider.requirements(&["asdf"]);
        let mut solver = Solver::new(provider);
        let solved = solver.solve(requirements).unwrap();

        assert_eq!(solved.len(), 1);

        let solvable = solver
            .pool()
            .resolve_internal_solvable(solved[0])
            .solvable();

        assert_eq!(solver.pool().resolve_package_name(solvable.name), "asdf");
        assert_eq!(solvable.inner.0, 3);
    }

    /// Locking a specific package version in this case a lower version namely `3` should result
    /// in the higher package not being considered
    #[test]
    fn test_resolve_locked_top_level() {
        let mut provider =
            BundleBoxProvider::from_packages(&[("asdf", 4, vec![]), ("asdf", 3, vec![])]);
        provider.set_locked("asdf", 3);

        let requirements = provider.requirements(&["asdf"]);

        let mut solver = Solver::new(provider);
        let solved = solver.solve(requirements).unwrap();

        assert_eq!(solved.len(), 1);
        let solvable_id = solved[0];
        assert_eq!(
            solver
                .pool()
                .resolve_internal_solvable(solvable_id)
                .solvable()
                .inner
                .0,
            3
        );
    }

    /// Should ignore lock when it is not a top level package and a newer version exists without it
    #[test]
    fn test_resolve_ignored_locked_top_level() {
        let mut provider = BundleBoxProvider::from_packages(&[
            ("asdf", 4, vec![]),
            ("asdf", 3, vec!["fgh"]),
            ("fgh", 1, vec![]),
        ]);

        provider.set_locked("fgh", 1);

        let requirements = provider.requirements(&["asdf"]);
        let mut solver = Solver::new(provider);
        let solved = solver.solve(requirements).unwrap();

        assert_eq!(solved.len(), 1);
        let solvable = solver
            .pool()
            .resolve_internal_solvable(solved[0])
            .solvable();

        assert_eq!(solver.pool().resolve_package_name(solvable.name), "asdf");
        assert_eq!(solvable.inner.0, 4);
    }

    /// Test checks if favoring without a conflict results in a package upgrade
    #[test]
    fn test_resolve_favor_without_conflict() {
        let mut provider = BundleBoxProvider::from_packages(&[
            ("a", 1, vec![]),
            ("a", 2, vec![]),
            ("b", 1, vec![]),
            ("b", 2, vec![]),
        ]);
        provider.set_favored("a", 1);
        provider.set_favored("b", 1);

        let requirements = provider.requirements(&["a", "b 2"]);

        // Already installed: A=1; B=1
        let mut solver = Solver::new(provider);
        let solved = solver.solve(requirements);
        let solved = match solved {
            Ok(solved) => solved,
            Err(p) => panic!(
                "{}",
                p.display_user_friendly(&solver, &DefaultSolvableDisplay)
            ),
        };

        let result = transaction_to_string(&solver.pool(), &solved);
        insta::assert_snapshot!(result, @r###"
        b=2
        a=1
        "###);
    }
    //
    #[test]
    fn test_resolve_favor_with_conflict() {
        let mut provider = BundleBoxProvider::from_packages(&[
            ("a", 1, vec!["c 1"]),
            ("a", 2, vec![]),
            ("b", 1, vec!["c 1"]),
            ("b", 2, vec!["c 2"]),
            ("c", 1, vec![]),
            ("c", 2, vec![]),
        ]);
        provider.set_favored("a", 1);
        provider.set_favored("b", 1);
        provider.set_favored("c", 1);

        let requirements = provider.requirements(&["a", "b 2"]);

        let mut solver = Solver::new(provider);
        let solved = solver.solve(requirements);
        let solved = match solved {
            Ok(solved) => solved,
            Err(p) => panic!(
                "{}",
                p.display_user_friendly(&solver, &DefaultSolvableDisplay)
            ),
        };

        let result = transaction_to_string(&solver.pool(), &solved);
        insta::assert_snapshot!(result, @r###"
        b=2
        c=2
        a=2
        "###);
    }

    #[test]
    fn test_resolve_cyclic() {
        let provider = BundleBoxProvider::from_packages(&[
            ("a", 2, vec!["b 0..10"]),
            ("b", 5, vec!["a 2..4"]),
        ]);
        let requirements = provider.requirements(&["a 0..100"]);
        let mut solver = Solver::new(provider);
        let solved = solver.solve(requirements).unwrap();

        let result = transaction_to_string(&solver.pool(), &solved);
        insta::assert_snapshot!(result, @r###"
        a=2
        b=5
        "###);
    }

    #[test]
    fn test_unsat_locked_and_excluded() {
        let mut provider = BundleBoxProvider::from_packages(&[
            ("asdf", 1, vec!["c 2"]),
            ("c", 2, vec![]),
            ("c", 1, vec![]),
        ]);
        provider.set_locked("c", 1);
        let error = solve_unsat(provider, &["asdf"]);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_no_candidates_for_child_1() {
        let provider =
            BundleBoxProvider::from_packages(&[("asdf", 1, vec!["c 2"]), ("c", 1, vec![])]);
        let error = solve_unsat(provider, &["asdf"]);
        insta::assert_snapshot!(error);
    }
    //
    #[test]
    fn test_unsat_no_candidates_for_child_2() {
        let provider = BundleBoxProvider::from_packages(&[("a", 41, vec!["B 0..20"])]);
        let error = solve_unsat(provider, &["a 0..1000"]);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_missing_top_level_dep_1() {
        let provider = BundleBoxProvider::from_packages(&[("asdf", 1, vec![])]);
        let error = solve_unsat(provider, &["fghj"]);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_missing_top_level_dep_2() {
        let provider =
            BundleBoxProvider::from_packages(&[("a", 41, vec!["b 15"]), ("b", 15, vec![])]);
        let error = solve_unsat(provider, &["a 41", "b 14"]);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_after_backtracking() {
        let provider = BundleBoxProvider::from_packages(&[
            ("b", 7, vec!["d 1"]),
            ("b", 6, vec!["d 1"]),
            ("c", 1, vec!["d 2"]),
            ("c", 2, vec!["d 2"]),
            ("d", 2, vec![]),
            ("d", 1, vec![]),
            ("e", 1, vec![]),
            ("e", 2, vec![]),
        ]);

        let error = solve_unsat(provider, &["b", "c", "e"]);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_incompatible_root_requirements() {
        let provider = BundleBoxProvider::from_packages(&[("a", 2, vec![]), ("a", 5, vec![])]);
        let error = solve_unsat(provider, &["a 0..4", "a 5..10"]);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_bluesky_conflict() {
        let provider = BundleBoxProvider::from_packages(&[
            ("suitcase-utils", 54, vec![]),
            ("suitcase-utils", 53, vec![]),
            (
                "bluesky-widgets",
                42,
                vec![
                    "bluesky-live 0..10",
                    "numpy 0..10",
                    "python 0..10",
                    "suitcase-utils 0..54",
                ],
            ),
            ("bluesky-live", 1, vec![]),
            ("numpy", 1, vec![]),
            ("python", 1, vec![]),
        ]);
        let error = solve_unsat(
            provider,
            &["bluesky-widgets 0..100", "suitcase-utils 54..100"],
        );
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_pubgrub_article() {
        // Taken from the pubgrub article: https://nex3.medium.com/pubgrub-2fb6470504f
        let provider = BundleBoxProvider::from_packages(&[
            ("menu", 15, vec!["dropdown 2..3"]),
            ("menu", 10, vec!["dropdown 1..2"]),
            ("dropdown", 2, vec!["icons 2"]),
            ("dropdown", 1, vec!["intl 3"]),
            ("icons", 2, vec![]),
            ("icons", 1, vec![]),
            ("intl", 5, vec![]),
            ("intl", 3, vec![]),
        ]);
        let error = solve_unsat(provider, &["menu", "icons 1", "intl 5"]);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_applies_graph_compression() {
        let provider = BundleBoxProvider::from_packages(&[
            ("a", 10, vec!["b"]),
            ("a", 9, vec!["b"]),
            ("b", 100, vec!["c 0..100"]),
            ("b", 42, vec!["c 0..100"]),
            ("c", 103, vec![]),
            ("c", 101, vec![]),
            ("c", 100, vec![]),
            ("c", 99, vec![]),
        ]);
        let error = solve_unsat(provider, &["a", "c 101..104"]);
        insta::assert_snapshot!(error);
    }
    //
    #[test]
    fn test_unsat_constrains() {
        let mut provider = BundleBoxProvider::from_packages(&[
            ("a", 10, vec!["b 50..100"]),
            ("a", 9, vec!["b 50..100"]),
            ("b", 50, vec![]),
            ("b", 42, vec![]),
        ]);

        provider.add_package("c", 10.into(), &vec![], &vec!["b 0..50"]);
        provider.add_package("c", 8.into(), &vec![], &vec!["b 0..50"]);
        let error = solve_unsat(provider, &["a", "c"]);
        insta::assert_snapshot!(error);
    }

    #[test]
    #[tracing_test::traced_test]
    fn test_unsat_constrains_2() {
        let mut provider = BundleBoxProvider::from_packages(&[
            ("a", 1, vec!["b"]),
            ("a", 2, vec!["b"]),
            ("b", 1, vec!["c 1"]),
            ("b", 2, vec!["c 2"]),
        ]);

        provider.add_package("c", 1.into(), &vec![], &vec!["a 3"]);
        provider.add_package("c", 2.into(), &vec![], &vec!["a 3"]);
        let error = solve_unsat(provider, &["a"]);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_missing_dep() {
        let provider =
            BundleBoxProvider::from_packages(&[("a", 2, vec!["missing"]), ("a", 1, vec![])]);
        let requirements = provider.requirements(&["a"]);
        let mut solver = Solver::new(provider);
        let result = match solver.solve(requirements) {
            Ok(result) => transaction_to_string(solver.pool(), &result),
            Err(problem) => problem
                .display_user_friendly(&solver, &DefaultSolvableDisplay)
                .to_string(),
        };
        insta::assert_snapshot!(result);
    }

    #[test]
    #[tracing_test::traced_test]
    fn test_no_backtracking() {
        let provider = BundleBoxProvider::from_packages(&[
            ("quetz-server", 2, vec!["pydantic 10..20"]),
            ("quetz-server", 1, vec!["pydantic 0..10"]),
            ("pydantic", 1, vec![]),
            ("pydantic", 2, vec![]),
            ("pydantic", 3, vec![]),
            ("pydantic", 4, vec![]),
            ("pydantic", 5, vec![]),
            ("pydantic", 6, vec![]),
            ("pydantic", 7, vec![]),
            ("pydantic", 8, vec![]),
            ("pydantic", 9, vec![]),
            ("pydantic", 10, vec![]),
            ("pydantic", 11, vec![]),
            ("pydantic", 12, vec![]),
            ("pydantic", 13, vec![]),
            ("pydantic", 14, vec![]),
        ]);
        let requirements = provider.requirements(&["quetz-server", "pydantic 0..10"]);
        let mut solver = Solver::new(provider);
        let solved = solver.solve(requirements).unwrap();
        insta::assert_snapshot!(transaction_to_string(solver.pool(), &solved));
    }
}
