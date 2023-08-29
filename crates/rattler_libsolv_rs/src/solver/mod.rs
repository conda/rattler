use crate::arena::{Arena, ArenaId};
use crate::id::{ClauseId, SolvableId};
use crate::id::{LearntClauseId, NameId};
use crate::mapping::Mapping;
use crate::pool::Pool;
use crate::problem::Problem;
use crate::solvable::SolvableInner;
use crate::solve_jobs::SolveJobs;
use crate::transaction::Transaction;
use std::cell::OnceCell;

use itertools::Itertools;
use rattler_conda_types::MatchSpec;
use std::collections::{HashMap, HashSet};

use crate::VersionSetId;
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
///
/// Keeps solvables in a `Pool`, which contains references to `PackageRecord`s (the `'a` lifetime
/// comes from the original `PackageRecord`s)
pub struct Solver<'a> {
    pool: Pool<'a, MatchSpec>,

    pub(crate) clauses: Vec<ClauseState>,
    watches: WatchMap,

    learnt_clauses_start: ClauseId,
    learnt_clauses: Arena<LearntClauseId, Vec<Literal>>,
    learnt_why: Mapping<LearntClauseId, Vec<ClauseId>>,

    decision_tracker: DecisionTracker,
}

impl<'a> Solver<'a> {
    /// Create a solver, using the provided pool
    pub fn new(pool: Pool<'a, MatchSpec>) -> Self {
        Self {
            clauses: Vec::new(),
            watches: WatchMap::new(),
            learnt_clauses: Arena::new(),
            learnt_clauses_start: ClauseId::null(),
            learnt_why: Mapping::empty(),
            decision_tracker: DecisionTracker::new(pool.solvables.len() as u32),
            pool,
        }
    }

    /// Returns a reference to the pool used by the solver
    pub fn pool(&self) -> &Pool<MatchSpec> {
        &self.pool
    }

    /// Solves the provided `jobs` and returns a transaction from the found solution
    ///
    /// Returns a [`Problem`] if no solution was found, which provides ways to inspect the causes
    /// and report them to the user.
    pub fn solve(&mut self, jobs: SolveJobs) -> Result<Transaction, Problem> {
        // Clear state
        self.pool.root_solvable_mut().clear();
        self.decision_tracker.clear();
        self.learnt_clauses.clear();
        self.learnt_why = Mapping::empty();
        self.clauses = vec![ClauseState::new(
            Clause::InstallRoot,
            &self.learnt_clauses,
            &self.pool.match_spec_to_sorted_candidates,
        )];

        // Favored map
        let mut favored_map = HashMap::new();
        for &favored_id in &jobs.favor {
            let name_id = self.pool.resolve_solvable_inner(favored_id).package().name;
            favored_map.insert(name_id, favored_id);
        }

        // Populate the root solvable with the requested packages
        for match_spec in jobs.install.iter() {
            self.pool.root_solvable_mut().push(*match_spec);
        }

        // Create clauses for root's dependencies, and their dependencies, and so forth
        self.add_clauses_for_root_deps(&favored_map);

        // Add clauses ensuring only a single candidate per package name is installed
        for candidates in self.pool.packages_by_name.values() {
            // Each candidate gets a clause with each other candidate
            for (i, &candidate) in candidates.iter().enumerate() {
                for &other_candidate in &candidates[i + 1..] {
                    self.clauses.push(ClauseState::new(
                        Clause::ForbidMultipleInstances(candidate, other_candidate),
                        &self.learnt_clauses,
                        &self.pool.match_spec_to_sorted_candidates,
                    ));
                }
            }
        }

        // Add clauses for the locked solvable
        for &locked_solvable_id in &jobs.lock {
            // For each locked solvable, forbid other solvables with the same name
            let name = self.pool.resolve_solvable(locked_solvable_id).name;
            for &other_candidate in &self.pool.packages_by_name[name] {
                if other_candidate != locked_solvable_id {
                    self.clauses.push(ClauseState::new(
                        Clause::Lock(locked_solvable_id, other_candidate),
                        &self.learnt_clauses,
                        &self.pool.match_spec_to_sorted_candidates,
                    ));
                }
            }
        }

        // All new clauses are learnt after this point
        self.learnt_clauses_start = ClauseId::new(self.clauses.len());

        // Create watches chains
        self.make_watches();

        // Run SAT
        self.run_sat(&jobs.install, &jobs.lock)?;

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
        Ok(Transaction { steps })
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
    fn add_clauses_for_root_deps(&mut self, favored_map: &HashMap<NameId, SolvableId>) {
        let mut visited = HashSet::new();
        let mut stack = Vec::new();

        stack.push(SolvableId::root());

        let mut match_spec_to_sorted_candidates =
            Mapping::new(vec![Vec::new(); self.pool.version_sets.len()]);
        let mut match_spec_to_forbidden =
            Mapping::new(vec![Vec::new(); self.pool.version_sets.len()]);
        let match_spec_to_candidates =
            Mapping::new(vec![OnceCell::new(); self.pool.version_sets.len()]);
        let match_spec_to_highest_version =
            Mapping::new(vec![OnceCell::new(); self.pool.version_sets.len()]);
        let mut sorting_cache = HashMap::new();
        let mut seen_requires = HashSet::new();
        let mut seen_forbidden = HashSet::new();
        let empty_vec = Vec::new();

        while let Some(solvable_id) = stack.pop() {
            let (deps, constrains) = match &self.pool.solvables[solvable_id].inner {
                SolvableInner::Root(deps) => (deps, &[] as &[_]),
                SolvableInner::Package(pkg) => (&pkg.dependencies, pkg.constrains.as_slice()),
            };

            // Enqueue the candidates of the dependencies
            for &dep in deps {
                if seen_requires.insert(dep) {
                    self.pool.populate_candidates(
                        dep,
                        favored_map,
                        &mut match_spec_to_sorted_candidates,
                        &match_spec_to_candidates,
                        &match_spec_to_highest_version,
                        &mut sorting_cache,
                    );
                }

                for &candidate in match_spec_to_sorted_candidates
                    .get(dep)
                    .unwrap_or(&empty_vec)
                {
                    // Note: we skip candidates we have already seen
                    if visited.insert(candidate) {
                        stack.push(candidate);
                    }
                }
            }

            // Requires
            for &dep in deps {
                self.clauses.push(ClauseState::new(
                    Clause::Requires(solvable_id, dep),
                    &self.learnt_clauses,
                    &match_spec_to_sorted_candidates,
                ));
            }

            // Constrains
            for &dep in constrains {
                if seen_forbidden.insert(dep) {
                    self.pool
                        .populate_forbidden(dep, &mut match_spec_to_forbidden);
                }

                for &solvable_dep in match_spec_to_forbidden.get(dep).unwrap_or(&empty_vec) {
                    self.clauses.push(ClauseState::new(
                        Clause::Constrains(solvable_id, solvable_dep, dep),
                        &self.learnt_clauses,
                        &match_spec_to_sorted_candidates,
                    ));
                }
            }
        }

        self.pool.match_spec_to_sorted_candidates = match_spec_to_sorted_candidates;
        self.pool.match_spec_to_forbidden = match_spec_to_forbidden;
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
    fn run_sat(
        &mut self,
        top_level_requirements: &[VersionSetId],
        locked_solvables: &[SolvableId],
    ) -> Result<(), Problem> {
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
        self.decide_requires_without_candidates(level, locked_solvables, top_level_requirements)
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
    fn decide_requires_without_candidates(
        &mut self,
        level: u32,
        _locked_solvables: &[SolvableId],
        _top_level_requirements: &[VersionSetId],
    ) -> Result<(), ClauseId> {
        tracing::info!("=== Deciding assertions for requires without candidates");

        for (i, clause) in self.clauses.iter().enumerate() {
            if let Clause::Requires(solvable_id, _) = clause.kind {
                if !clause.has_watches() {
                    // A requires clause without watches means it has a single literal (i.e.
                    // there are no candidates)
                    let clause_id = ClauseId::new(i);
                    let decided = self
                        .decision_tracker
                        .try_add_decision(Decision::new(solvable_id, false, clause_id), level)
                        .map_err(|_| clause_id)?;

                    if decided {
                        tracing::info!(
                            "Set {} = false",
                            self.pool.resolve_solvable_inner(solvable_id).display()
                        );
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

            let (required_by, candidate) = {
                let clause = &self.clauses[i];
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
                let candidates = &self.pool.match_spec_to_sorted_candidates[deps];
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

            level = self.set_propagate_learn(level, candidate, required_by, ClauseId::new(i))?;

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
            "=== Install {} at level {level} (required by {})",
            self.pool.resolve_solvable_inner(solvable).display(),
            self.pool.resolve_solvable_inner(required_by).display(),
        );

        self.decision_tracker
            .try_add_decision(Decision::new(solvable, true, clause_id), level)
            .expect("bug: solvable was already decided!");

        loop {
            let r = self.propagate(level);
            let Err((conflicting_solvable, attempted_value, conflicting_clause)) = r else {
                // Propagation succeeded
                tracing::info!("=== Propagation succeeded");
                break;
            };

            {
                tracing::info!(
                    "=== Propagation conflicted: could not set {solvable} to {attempted_value}",
                    solvable = self
                        .pool
                        .resolve_solvable_inner(conflicting_solvable)
                        .display()
                );
                tracing::info!(
                    "During unit propagation for clause: {:?}",
                    self.clauses[conflicting_clause.index()].debug(&self.pool)
                );

                tracing::info!(
                    "Previously decided value: {}. Derived from: {:?}",
                    !attempted_value,
                    self.clauses[self
                        .decision_tracker
                        .stack()
                        .iter()
                        .find(|d| d.solvable_id == conflicting_solvable)
                        .unwrap()
                        .derived_from
                        .index()]
                    .debug(&self.pool),
                );
            }

            if level == 1 {
                tracing::info!("=== UNSOLVABLE");
                for decision in self.decision_tracker.stack() {
                    let clause = &self.clauses[decision.derived_from.index()];
                    let level = self.decision_tracker.level(decision.solvable_id);
                    let action = if decision.value { "install" } else { "forbid" };

                    if let Clause::ForbidMultipleInstances(..) = clause.kind {
                        // Skip forbids clauses, to reduce noise
                        continue;
                    }

                    tracing::info!(
                        "* ({level}) {action} {}. Reason: {:?}",
                        self.pool
                            .resolve_solvable_inner(decision.solvable_id)
                            .display(),
                        clause.debug(&self.pool),
                    );
                }

                return Err(self.analyze_unsolvable(conflicting_clause));
            }

            let (new_level, learned_clause_id, literal) =
                self.analyze(level, conflicting_solvable, conflicting_clause);
            level = new_level;

            tracing::info!("=== Backtracked to level {level}");

            // Optimization: propagate right now, since we know that the clause is a unit clause
            let decision = literal.satisfying_value();
            self.decision_tracker
                .try_add_decision(
                    Decision::new(literal.solvable_id, decision, learned_clause_id),
                    level,
                )
                .expect("bug: solvable was already decided!");
            tracing::info!(
                "=== Propagate after learn: {} = {decision}",
                self.pool
                    .resolve_solvable_inner(literal.solvable_id)
                    .display()
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
        let learnt_clauses_start = self.learnt_clauses_start.index();
        for (i, clause) in self.clauses[learnt_clauses_start..].iter().enumerate() {
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
            let clause_id = ClauseId::new(learnt_clauses_start + i);

            let decided = self
                .decision_tracker
                .try_add_decision(
                    Decision::new(literal.solvable_id, decision, clause_id),
                    level,
                )
                .map_err(|_| (literal.solvable_id, decision, clause_id))?;

            if decided {
                tracing::info!(
                    "Propagate assertion {} = {}",
                    self.pool
                        .resolve_solvable_inner(literal.solvable_id)
                        .display(),
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

                // This is a convoluted way of getting mutable access to the current and the previous clause,
                // which is necessary when we have to remove the current clause from the list
                let (predecessor_clause, clause) =
                    if let Some(prev_clause_id) = predecessor_clause_id {
                        if prev_clause_id < clause_id {
                            let (prev, current) = self.clauses.split_at_mut(clause_id.index());
                            (Some(&mut prev[prev_clause_id.index()]), &mut current[0])
                        } else {
                            let (current, prev) = self.clauses.split_at_mut(prev_clause_id.index());
                            (Some(&mut prev[0]), &mut current[clause_id.index()])
                        }
                    } else {
                        (None, &mut self.clauses[clause_id.index()])
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
                        &self.pool,
                        &self.learnt_clauses,
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
                                        "Propagate {} = {}. {:?}",
                                        self.pool
                                            .resolve_solvable_inner(remaining_watch.solvable_id)
                                            .display(),
                                        remaining_watch.satisfying_value(),
                                        clause.debug(&self.pool),
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
        clauses: &[ClauseState],
        learnt_why: &Mapping<LearntClauseId, Vec<ClauseId>>,
        learnt_clauses_start: ClauseId,
        clause_id: ClauseId,
        problem: &mut Problem,
        seen: &mut HashSet<ClauseId>,
    ) {
        let clause = &clauses[clause_id.index()];
        match clause.kind {
            Clause::Learnt(..) => {
                if !seen.insert(clause_id) {
                    return;
                }

                let clause_id =
                    LearntClauseId::from_usize(clause_id.index() - learnt_clauses_start.index());
                for &cause in &learnt_why[clause_id] {
                    Self::analyze_unsolvable_clause(
                        clauses,
                        learnt_why,
                        learnt_clauses_start,
                        cause,
                        problem,
                        seen,
                    );
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
        self.clauses[clause_id.index()].kind.visit_literals(
            &self.learnt_clauses,
            &self.pool,
            |literal| {
                involved.insert(literal.solvable_id);
            },
        );

        let mut seen = HashSet::new();
        Self::analyze_unsolvable_clause(
            &self.clauses,
            &self.learnt_why,
            self.learnt_clauses_start,
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
                self.learnt_clauses_start,
                why,
                &mut problem,
                &mut seen,
            );

            self.clauses[why.index()].kind.visit_literals(
                &self.learnt_clauses,
                &self.pool,
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

            self.clauses[clause_id.index()].kind.visit_literals(
                &self.learnt_clauses,
                &self.pool,
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
        let clause_id = ClauseId::new(self.clauses.len());
        let learnt_id = self.learnt_clauses.alloc(learnt.clone());
        self.learnt_why.extend(learnt_why);

        let mut clause = ClauseState::new(
            Clause::Learnt(learnt_id),
            &self.learnt_clauses,
            &self.pool.match_spec_to_sorted_candidates,
        );

        if clause.has_watches() {
            self.watches.start_watching(&mut clause, clause_id);
        }

        // Store it
        self.clauses.push(clause);

        tracing::info!(
            "Learnt disjunction:\n{}",
            learnt
                .into_iter()
                .format_with("\n", |lit, f| f(&format_args!(
                    "- {}{}",
                    if lit.negate { "NOT " } else { "" },
                    self.pool.resolve_solvable_inner(lit.solvable_id).display()
                )))
        );

        // Should revert at most to the root level
        let target_level = back_track_to.max(1);
        self.decision_tracker.undo_until(target_level);

        (target_level, clause_id, last_literal)
    }

    fn make_watches(&mut self) {
        self.watches.initialize(self.pool.solvables.len());

        // Watches are already initialized in the clauses themselves, here we build a linked list for
        // each package (a clause will be linked to other clauses that are watching the same package)
        for (i, clause) in self.clauses.iter_mut().enumerate() {
            if !clause.has_watches() {
                // Skip clauses without watches
                continue;
            }

            self.watches.start_watching(clause, ClauseId::new(i));
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::id::RepoId;
    use crate::pool::VersionSet;
    use rattler_conda_types::{PackageRecord, Version};
    use std::fmt::Debug;
    use std::str::FromStr;

    fn package(name: &str, version: &str, deps: &[&str], constrains: &[&str]) -> PackageRecord {
        PackageRecord {
            arch: None,
            build: "".to_string(),
            build_number: 0,
            constrains: constrains.iter().map(|s| s.to_string()).collect(),
            depends: deps.iter().map(|s| s.to_string()).collect(),
            features: None,
            legacy_bz2_md5: None,
            legacy_bz2_size: None,
            license: None,
            license_family: None,
            md5: None,
            name: name.parse().unwrap(),
            noarch: Default::default(),
            platform: None,
            sha256: None,
            size: None,
            subdir: "".to_string(),
            timestamp: None,
            track_features: vec![],
            version: version.parse().unwrap(),
        }
    }

    fn add_package(pool: &mut Pool<MatchSpec>, record: PackageRecord) {
        let record = Box::leak(Box::new(record));
        let solvable_id = pool.add_package(RepoId::new(0), record);

        for dep in &record.depends {
            pool.add_dependency(solvable_id, MatchSpec::from_str(dep).unwrap());
        }

        for constrain in &record.constrains {
            pool.add_constrains(solvable_id, MatchSpec::from_str(constrain).unwrap());
        }
    }

    fn pool(packages: &[(&str, &str, Vec<&str>)]) -> Pool<'static, MatchSpec> {
        let mut pool = Pool::new();
        for (pkg_name, version, deps) in packages {
            let pkg_name = *pkg_name;
            let version = *version;
            let record = package(pkg_name, version, deps, &[]);
            add_package(&mut pool, record);
        }

        pool
    }

    fn install<V: VersionSet + FromStr>(pool: &mut Pool<'static, V>, packages: &[&str]) -> SolveJobs
    where
        <V as FromStr>::Err: Debug,
    {
        let mut jobs = SolveJobs::default();
        for &p in packages {
            let version_set_id = pool.intern_version_set(p.parse().unwrap());
            jobs.install(version_set_id);
        }
        jobs
    }

    fn transaction_to_string(pool: &Pool<MatchSpec>, transaction: &Transaction) -> String {
        use std::fmt::Write;
        let mut buf = String::new();
        for &solvable_id in &transaction.steps {
            writeln!(
                buf,
                "{}",
                pool.resolve_solvable_inner(solvable_id).display()
            )
            .unwrap();
        }

        buf
    }

    fn solve_unsat(pool: Pool<MatchSpec>, jobs: SolveJobs) -> String {
        let mut solver = Solver::new(pool);
        match solver.solve(jobs) {
            Ok(_) => panic!("expected unsat, but a solution was found"),
            Err(problem) => problem.display_user_friendly(&solver).to_string(),
        }
    }

    #[test]
    fn test_unit_propagation_1() {
        let mut pool = pool(&[("asdf", "1.2.3", vec![])]);
        let jobs = install(&mut pool, &["asdf"]);
        let mut solver = Solver::new(pool);
        let solved = solver.solve(jobs).unwrap();

        assert_eq!(solved.steps.len(), 1);

        let solvable = solver
            .pool
            .resolve_solvable_inner(solved.steps[0])
            .package();
        assert_eq!(solvable.record.name.as_normalized(), "asdf");
        assert_eq!(solvable.record.version.to_string(), "1.2.3");
    }

    #[test]
    fn test_unit_propagation_nested() {
        let mut pool = pool(&[
            ("asdf", "1.2.3", vec!["efgh"]),
            ("efgh", "4.5.6", vec![]),
            ("dummy", "42.42.42", vec![]),
        ]);
        let jobs = install(&mut pool, &["asdf"]);
        let mut solver = Solver::new(pool);
        let solved = solver.solve(jobs).unwrap();

        assert_eq!(solved.steps.len(), 2);

        let solvable = solver
            .pool
            .resolve_solvable_inner(solved.steps[0])
            .package();
        assert_eq!(solvable.record.name.as_normalized(), "asdf");
        assert_eq!(solvable.record.version.to_string(), "1.2.3");

        let solvable = solver
            .pool
            .resolve_solvable_inner(solved.steps[1])
            .package();
        assert_eq!(solvable.record.name.as_normalized(), "efgh");
        assert_eq!(solvable.record.version.to_string(), "4.5.6");
    }

    #[test]
    fn test_resolve_dependencies() {
        let mut pool = pool(&[
            ("asdf", "1.2.4", vec![]),
            ("asdf", "1.2.3", vec![]),
            ("efgh", "4.5.7", vec![]),
            ("efgh", "4.5.6", vec![]),
        ]);
        let jobs = install(&mut pool, &["asdf", "efgh"]);
        let mut solver = Solver::new(pool);
        let solved = solver.solve(jobs).unwrap();

        assert_eq!(solved.steps.len(), 2);

        let solvable = solver
            .pool
            .resolve_solvable_inner(solved.steps[0])
            .package();
        assert_eq!(solvable.record.name.as_normalized(), "asdf");
        assert_eq!(solvable.record.version.to_string(), "1.2.4");

        let solvable = solver
            .pool
            .resolve_solvable_inner(solved.steps[1])
            .package();
        assert_eq!(solvable.record.name.as_normalized(), "efgh");
        assert_eq!(solvable.record.version.to_string(), "4.5.7");
    }

    #[test]
    fn test_resolve_with_conflict() {
        let mut pool = pool(&[
            ("asdf", "1.2.4", vec!["conflicting=1.0.1"]),
            ("asdf", "1.2.3", vec!["conflicting=1.0.0"]),
            ("efgh", "4.5.7", vec!["conflicting=1.0.0"]),
            ("efgh", "4.5.6", vec!["conflicting=1.0.0"]),
            ("conflicting", "1.0.1", vec![]),
            ("conflicting", "1.0.0", vec![]),
        ]);
        let jobs = install(&mut pool, &["asdf", "efgh"]);
        let mut solver = Solver::new(pool);
        let solved = solver.solve(jobs).unwrap();

        use std::fmt::Write;
        let mut display_result = String::new();
        for &solvable_id in &solved.steps {
            let solvable = solver.pool().resolve_solvable_inner(solvable_id).display();
            writeln!(display_result, "{solvable}").unwrap();
        }

        insta::assert_snapshot!(display_result);
    }

    #[test]
    fn test_resolve_with_nonexisting() {
        let mut pool = pool(&[
            ("asdf", "1.2.4", vec!["b"]),
            ("asdf", "1.2.3", vec![]),
            ("b", "1.2.3", vec!["idontexist"]),
        ]);
        let jobs = install(&mut pool, &["asdf"]);
        let mut solver = Solver::new(pool);
        let solved = solver.solve(jobs).unwrap();

        assert_eq!(solved.steps.len(), 1);

        let solvable = solver
            .pool
            .resolve_solvable_inner(solved.steps[0])
            .package();
        assert_eq!(solvable.record.name.as_normalized(), "asdf");
        assert_eq!(solvable.record.version.to_string(), "1.2.3");
    }

    #[test]
    fn test_resolve_locked_top_level() {
        let mut pool = pool(&[("asdf", "1.2.4", vec![]), ("asdf", "1.2.3", vec![])]);

        let locked = pool
            .solvables
            .as_slice()
            .iter()
            .position(|s| {
                if let Some(package) = s.get_package() {
                    package.record.version == Version::from_str("1.2.3").unwrap()
                } else {
                    false
                }
            })
            .unwrap();

        let locked = SolvableId::from_usize(locked);

        let mut jobs = install(&mut pool, &["asdf"]);
        jobs.lock(locked);

        let mut solver = Solver::new(pool);
        let solved = solver.solve(jobs).unwrap();

        assert_eq!(solved.steps.len(), 1);
        let solvable_id = solved.steps[0];
        assert_eq!(solvable_id, locked);
    }

    #[test]
    fn test_resolve_ignored_locked_top_level() {
        let mut pool = pool(&[
            ("asdf", "1.2.4", vec![]),
            ("asdf", "1.2.3", vec!["fgh"]),
            ("fgh", "1.0.0", vec![]),
        ]);

        let locked = pool
            .solvables
            .as_slice()
            .iter()
            .position(|s| {
                if let Some(package) = s.get_package() {
                    package.record.version == Version::from_str("1.0.0").unwrap()
                } else {
                    false
                }
            })
            .unwrap();

        let locked = SolvableId::from_usize(locked);

        let mut jobs = install(&mut pool, &["asdf"]);
        jobs.lock(locked);

        let mut solver = Solver::new(pool);
        let solved = solver.solve(jobs).unwrap();

        assert_eq!(solved.steps.len(), 1);
        let solvable = solver
            .pool
            .resolve_solvable_inner(solved.steps[0])
            .package();
        assert_eq!(solvable.record.name.as_normalized(), "asdf");
        assert_eq!(solvable.record.version, Version::from_str("1.2.4").unwrap());
    }

    #[test]
    fn test_resolve_favor_without_conflict() {
        let mut pool = pool(&[
            ("a", "1", vec![]),
            ("a", "2", vec![]),
            ("b", "1", vec![]),
            ("b", "2", vec![]),
        ]);

        let mut jobs = install(&mut pool, &["a", "b>=2"]);

        // Already installed: A=1; B=1
        let already_installed = pool
            .solvables
            .as_slice()
            .iter()
            .enumerate()
            .skip(1) // Skip the root solvable
            .filter(|(_, s)| s.package().record.version == Version::from_str("1").unwrap())
            .map(|(i, _)| SolvableId::from_usize(i));

        for solvable_id in already_installed {
            jobs.favor(solvable_id);
        }

        let mut solver = Solver::new(pool);
        let solved = solver.solve(jobs).unwrap();

        let result = transaction_to_string(&solver.pool, &solved);
        insta::assert_snapshot!(result, @r###"
        b 2
        a 1
        "###);
    }

    #[test]
    fn test_resolve_favor_with_conflict() {
        let mut pool = pool(&[
            ("a", "1", vec!["c=1"]),
            ("a", "2", vec![]),
            ("b", "1", vec!["c=1"]),
            ("b", "2", vec!["c=2"]),
            ("c", "1", vec![]),
            ("c", "2", vec![]),
        ]);

        let mut jobs = install(&mut pool, &["a", "b>=2"]);

        // Already installed: A=1; B=1; C=1
        let already_installed = pool
            .solvables
            .as_slice()
            .iter()
            .enumerate()
            .skip(1) // Skip the root solvable
            .filter(|(_, s)| s.package().record.version == Version::from_str("1").unwrap())
            .map(|(i, _)| SolvableId::from_usize(i));

        for solvable_id in already_installed {
            jobs.favor(solvable_id);
        }

        let mut solver = Solver::new(pool);
        let solved = solver.solve(jobs).unwrap();

        let result = transaction_to_string(&solver.pool, &solved);
        insta::assert_snapshot!(result, @r###"
        b 2
        c 2
        a 2
        "###);
    }

    #[test]
    fn test_resolve_cyclic() {
        let mut pool = pool(&[("a", "2", vec!["b<=10"]), ("b", "5", vec!["a>=2,<=4"])]);
        let jobs = install(&mut pool, &["a<100"]);
        let mut solver = Solver::new(pool);
        let solved = solver.solve(jobs).unwrap();

        let result = transaction_to_string(&solver.pool, &solved);
        insta::assert_snapshot!(result, @r###"
        a 2
        b 5
        "###);
    }

    #[test]
    fn test_unsat_locked_and_excluded() {
        let mut pool = pool(&[
            ("asdf", "1.2.3", vec!["c>1"]),
            ("c", "2.0.0", vec![]),
            ("c", "1.0.0", vec![]),
        ]);
        let mut job = install(&mut pool, &["asdf"]);
        job.lock(SolvableId::from_usize(3)); // C 1.0.0

        let error = solve_unsat(pool, job);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_no_candidates_for_child_1() {
        let mut pool = pool(&[("asdf", "1.2.3", vec!["C>1"]), ("C", "1.0.0", vec![])]);
        let jobs = install(&mut pool, &["asdf"]);
        let error = solve_unsat(pool, jobs);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_no_candidates_for_child_2() {
        let mut pool = pool(&[("a", "41", vec!["B<20"])]);
        let jobs = install(&mut pool, &["A<1000"]);

        let error = solve_unsat(pool, jobs);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_missing_top_level_dep_1() {
        let mut pool = pool(&[("asdf", "1.2.3", vec![])]);
        let jobs = install(&mut pool, &["fghj"]);
        let error = solve_unsat(pool, jobs);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_missing_top_level_dep_2() {
        let mut pool = pool(&[("a", "41", vec!["b=15"]), ("b", "15", vec![])]);
        let jobs = install(&mut pool, &["a=41", "b=14"]);

        let error = solve_unsat(pool, jobs);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_after_backtracking() {
        let mut pool = pool(&[
            ("b", "4.5.7", vec!["d=1"]),
            ("b", "4.5.6", vec!["d=1"]),
            ("c", "1.0.1", vec!["d=2"]),
            ("c", "1.0.0", vec!["d=2"]),
            ("d", "2.0.0", vec![]),
            ("d", "1.0.0", vec![]),
            ("e", "1.0.0", vec![]),
            ("e", "1.0.1", vec![]),
        ]);

        let jobs = install(&mut pool, &["b", "c", "e"]);
        let error = solve_unsat(pool, jobs);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_incompatible_root_requirements() {
        let mut pool = pool(&[("a", "2", vec![]), ("a", "5", vec![])]);
        let jobs = install(&mut pool, &["a<4", "a>=5,<10"]);

        let error = solve_unsat(pool, jobs);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_bluesky_conflict() {
        let mut pool = pool(&[
            ("suitcase-utils", "54", vec![]),
            ("suitcase-utils", "53", vec![]),
            (
                "bluesky-widgets",
                "42",
                vec![
                    "bluesky-live<10",
                    "numpy<10",
                    "python<10",
                    "suitcase-utils<54",
                ],
            ),
            ("bluesky-live", "1", vec![]),
            ("numpy", "1", vec![]),
            ("python", "1", vec![]),
        ]);

        let jobs = install(
            &mut pool,
            &["bluesky-widgets<100", "suitcase-utils>=54,<100"],
        );

        let error = solve_unsat(pool, jobs);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_pubgrub_article() {
        // Taken from the pubgrub article: https://nex3.medium.com/pubgrub-2fb6470504f
        let mut pool = pool(&[
            ("menu", "1.5.0", vec!["dropdown>=2.0.0,<=2.3.0"]),
            ("menu", "1.0.0", vec!["dropdown>=1.8.0,<2.0.0"]),
            ("dropdown", "2.3.0", vec!["icons=2.0.0"]),
            ("dropdown", "1.8.0", vec!["intl=3.0.0"]),
            ("icons", "2.0.0", vec![]),
            ("icons", "1.0.0", vec![]),
            ("intl", "5.0.0", vec![]),
            ("intl", "3.0.0", vec![]),
        ]);

        let jobs = install(&mut pool, &["menu", "icons=1.0.0", "intl=5.0.0"]);

        let error = solve_unsat(pool, jobs);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_applies_graph_compression() {
        let mut pool = pool(&[
            ("a", "10", vec!["b"]),
            ("a", "9", vec!["b"]),
            ("b", "100", vec!["c<100"]),
            ("b", "42", vec!["c<100"]),
            ("c", "103", vec![]),
            ("c", "101", vec![]),
            ("c", "100", vec![]),
            ("c", "99", vec![]),
        ]);

        let jobs = install(&mut pool, &["a", "c>100"]);

        let error = solve_unsat(pool, jobs);
        insta::assert_snapshot!(error);
    }

    #[test]
    fn test_unsat_constrains() {
        let mut pool = pool(&[
            ("a", "10", vec!["b>=50"]),
            ("a", "9", vec!["b>=50"]),
            ("b", "50", vec![]),
            ("b", "42", vec![]),
        ]);

        add_package(&mut pool, package("c", "10", &[], &["b<50"]));
        add_package(&mut pool, package("c", "8", &[], &["b<50"]));

        let jobs = install(&mut pool, &["a", "c"]);

        let error = solve_unsat(pool, jobs);
        insta::assert_snapshot!(error);
    }
}
