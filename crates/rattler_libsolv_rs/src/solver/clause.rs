use crate::arena::Arena;
use crate::id::ClauseId;
use crate::id::SolvableId;
use crate::id::{LearntClauseId, VersionSetId};
use crate::mapping::Mapping;
use crate::pool::Pool;
use crate::solver::decision_map::DecisionMap;
use crate::{Record, VersionSet};

use std::fmt::{Debug, Formatter};

/// Represents a single clause in the SAT problem
///
/// # SAT terminology
///
/// Clauses consist of disjunctions of literals (i.e. a non-empty list of variables, potentially
/// negated, joined by the logical "or" operator). Here are some examples:
///
/// - (¬A ∨ ¬B)
/// - (¬A ∨ ¬B ∨ ¬C ∨ ¬D)
/// - (¬A ∨ B ∨ C)
/// - (root)
///
/// For additional clarity: if `(¬A ∨ ¬B)` is a clause, `¬A` and `¬B` are its literals, and `A` and
/// `B` are variables. In our implementation, variables are represented by [`SolvableId`], and
/// assignments are tracked in the [`DecisionMap`].
///
/// The solver will attempt to assign values to the variables involved in the problem in such a way
/// that all clauses become true. If that turns out to be impossible, the problem is unsatisfiable.
///
/// Since we are not interested in general-purpose SAT solving, but are targeting the specific
/// use-case of dependency resolution, we only support a limited set of clauses. There are thousands
/// of clauses for a particular dependency resolution problem, and we try to keep the [`Clause`] enum
/// small. A naive implementation would store a `Vec<Literal>`.
#[derive(Copy, Clone, Debug)]
pub(crate) enum Clause {
    /// An assertion that the root solvable must be installed
    ///
    /// In SAT terms: (root)
    InstallRoot,
    /// The solvable requires the candidates associated to the match spec
    ///
    /// In SAT terms: (¬A ∨ B1 ∨ B2 ∨ ... ∨ B99), where B1 to B99 represent the possible candidates
    /// for the provided match spec
    Requires(SolvableId, VersionSetId),
    /// Ensures only a single version of a package is installed
    ///
    /// Usage: generate one [`Clause::ForbidMultipleInstances`] clause for each possible combination of
    /// packages under the same name. The clause itself forbids two solvables from being installed at
    /// the same time.
    ///
    /// In SAT terms: (¬A ∨ ¬B)
    ForbidMultipleInstances(SolvableId, SolvableId),
    /// Forbids packages that do not satisfy a solvable's constrains
    ///
    /// Usage: for each constrains relationship in a package, determine all the candidates that do
    /// _not_ satisfy it, and create one [`Clause::Constrains`]. The clause itself forbids two solvables
    /// from being installed at the same time, just as [`Clause::ForbidMultipleInstances`], but it
    /// pays off to have a separate variant for user-friendly error messages.
    ///
    /// In SAT terms: (¬A ∨ ¬B)
    Constrains(SolvableId, SolvableId, VersionSetId),
    /// Forbids the package on the right-hand side
    ///
    /// Note that the package on the left-hand side is not part of the clause, but just context to
    /// know which exact package was locked (necessary for user-friendly error messages)
    ///
    /// In SAT terms: (¬root ∨ ¬B). Note that we could encode this as an assertion (¬B), but that
    /// would require additional logic in the solver.
    Lock(SolvableId, SolvableId),
    /// A clause learnt during solving
    ///
    /// The learnt clause id can be used to retrieve the clause's literals, which are stored
    /// elsewhere to prevent the size of [`Clause`] from blowing up
    Learnt(LearntClauseId),
}

impl Clause {
    /// Returns the ids of the solvables that will be watched right after the clause is created
    fn initial_watches(
        &self,
        learnt_clauses: &Arena<LearntClauseId, Vec<Literal>>,
        match_spec_to_candidates: &Mapping<VersionSetId, Vec<SolvableId>>,
    ) -> Option<[SolvableId; 2]> {
        match self {
            Clause::InstallRoot => None,
            Clause::Constrains(s1, s2, _) | Clause::ForbidMultipleInstances(s1, s2) => {
                Some([*s1, *s2])
            }
            Clause::Lock(_, s) => Some([SolvableId::root(), *s]),
            &Clause::Learnt(learnt_id) => {
                let literals = &learnt_clauses[learnt_id];
                debug_assert!(!literals.is_empty());
                if literals.len() == 1 {
                    // No need for watches, since we learned an assertion
                    None
                } else {
                    Some([
                        literals.first().unwrap().solvable_id,
                        literals.last().unwrap().solvable_id,
                    ])
                }
            }
            &Clause::Requires(id, match_spec) => {
                let candidates = &match_spec_to_candidates[match_spec];
                if candidates.is_empty() {
                    None
                } else {
                    Some([id, candidates[0]])
                }
            }
        }
    }

    /// Visits each literal in the clause
    pub fn visit_literals<VS: VersionSet>(
        &self,
        learnt_clauses: &Arena<LearntClauseId, Vec<Literal>>,
        pool: &Pool<VS>,
        mut visit: impl FnMut(Literal),
    ) {
        match *self {
            Clause::InstallRoot => unreachable!(),
            Clause::Learnt(learnt_id) => {
                for &literal in &learnt_clauses[learnt_id] {
                    visit(literal);
                }
            }
            Clause::Requires(solvable_id, match_spec_id) => {
                visit(Literal {
                    solvable_id,
                    negate: true,
                });

                for &solvable_id in &pool.match_spec_to_sorted_candidates[match_spec_id] {
                    visit(Literal {
                        solvable_id,
                        negate: false,
                    });
                }
            }
            Clause::Constrains(s1, s2, _) | Clause::ForbidMultipleInstances(s1, s2) => {
                visit(Literal {
                    solvable_id: s1,
                    negate: true,
                });

                visit(Literal {
                    solvable_id: s2,
                    negate: true,
                });
            }
            Clause::Lock(_, s) => {
                visit(Literal {
                    solvable_id: SolvableId::root(),
                    negate: true,
                });

                visit(Literal {
                    solvable_id: s,
                    negate: true,
                });
            }
        }
    }
}

/// Keeps track of the literals watched by a [`Clause`] and the state associated to two linked lists
/// this clause is part of
///
/// In our SAT implementation, each clause tracks two literals present in its clause, to be notified
/// when the value assigned to the variable has changed (this technique is known as _watches_).
/// Clauses that are tracking the same variable are grouped together in a linked list, so it becomes
/// easy to notify them all.
#[derive(Clone)]
pub(crate) struct ClauseState {
    // The ids of the solvables this clause is watching
    pub watched_literals: [SolvableId; 2],
    // The ids of the next clause in each linked list that this clause is part of
    next_watches: [ClauseId; 2],
    // The clause itself
    pub(crate) kind: Clause,
}

impl ClauseState {
    pub fn new(
        kind: Clause,
        learnt_clauses: &Arena<LearntClauseId, Vec<Literal>>,
        match_spec_to_candidates: &Mapping<VersionSetId, Vec<SolvableId>>,
    ) -> Self {
        let watched_literals = kind
            .initial_watches(learnt_clauses, match_spec_to_candidates)
            .unwrap_or([SolvableId::null(), SolvableId::null()]);

        let clause = Self {
            watched_literals,
            next_watches: [ClauseId::null(), ClauseId::null()],
            kind,
        };

        debug_assert!(!clause.has_watches() || watched_literals[0] != watched_literals[1]);

        clause
    }

    pub fn debug<'a, VS: VersionSet>(&self, pool: &'a Pool<VS>) -> ClauseDebug<'a, VS> {
        ClauseDebug {
            kind: self.kind,
            pool,
        }
    }

    pub fn link_to_clause(&mut self, watch_index: usize, linked_clause: ClauseId) {
        self.next_watches[watch_index] = linked_clause;
    }

    pub fn get_linked_clause(&self, watch_index: usize) -> ClauseId {
        self.next_watches[watch_index]
    }

    pub fn unlink_clause(
        &mut self,
        linked_clause: &ClauseState,
        watched_solvable: SolvableId,
        linked_clause_watch_index: usize,
    ) {
        if self.watched_literals[0] == watched_solvable {
            self.next_watches[0] = linked_clause.next_watches[linked_clause_watch_index];
        } else {
            debug_assert_eq!(self.watched_literals[1], watched_solvable);
            self.next_watches[1] = linked_clause.next_watches[linked_clause_watch_index];
        }
    }

    #[inline]
    pub fn next_watched_clause(&self, solvable_id: SolvableId) -> ClauseId {
        if solvable_id == self.watched_literals[0] {
            self.next_watches[0]
        } else {
            debug_assert_eq!(self.watched_literals[1], solvable_id);
            self.next_watches[1]
        }
    }

    // Returns the index of the watch that turned false, if any
    pub fn watch_turned_false(
        &self,
        solvable_id: SolvableId,
        decision_map: &DecisionMap,
        learnt_clauses: &Arena<LearntClauseId, Vec<Literal>>,
    ) -> Option<([Literal; 2], usize)> {
        debug_assert!(self.watched_literals.contains(&solvable_id));

        let literals @ [w1, w2] = self.watched_literals(learnt_clauses);

        if solvable_id == w1.solvable_id && w1.eval(decision_map) == Some(false) {
            Some((literals, 0))
        } else if solvable_id == w2.solvable_id && w2.eval(decision_map) == Some(false) {
            Some((literals, 1))
        } else {
            None
        }
    }

    pub fn has_watches(&self) -> bool {
        // If the first watch is not null, the second won't be either
        !self.watched_literals[0].is_null()
    }

    pub fn watched_literals(
        &self,
        learnt_clauses: &Arena<LearntClauseId, Vec<Literal>>,
    ) -> [Literal; 2] {
        let literals = |op1: bool, op2: bool| {
            [
                Literal {
                    solvable_id: self.watched_literals[0],
                    negate: !op1,
                },
                Literal {
                    solvable_id: self.watched_literals[1],
                    negate: !op2,
                },
            ]
        };

        match self.kind {
            Clause::InstallRoot => unreachable!(),
            Clause::Learnt(learnt_id) => {
                // TODO: we might want to do something else for performance, like keeping the whole
                // literal in `self.watched_literals`, to avoid lookups... But first we should
                // benchmark!
                let &w1 = learnt_clauses[learnt_id]
                    .iter()
                    .find(|l| l.solvable_id == self.watched_literals[0])
                    .unwrap();
                let &w2 = learnt_clauses[learnt_id]
                    .iter()
                    .find(|l| l.solvable_id == self.watched_literals[1])
                    .unwrap();
                [w1, w2]
            }
            Clause::Constrains(..) | Clause::ForbidMultipleInstances(..) | Clause::Lock(..) => {
                literals(false, false)
            }
            Clause::Requires(solvable_id, _) => {
                if self.watched_literals[0] == solvable_id {
                    literals(false, true)
                } else if self.watched_literals[1] == solvable_id {
                    literals(true, false)
                } else {
                    literals(true, true)
                }
            }
        }
    }

    pub fn next_unwatched_variable<VS: VersionSet>(
        &self,
        pool: &Pool<VS>,
        learnt_clauses: &Arena<LearntClauseId, Vec<Literal>>,
        decision_map: &DecisionMap,
    ) -> Option<SolvableId> {
        // The next unwatched variable (if available), is a variable that is:
        // * Not already being watched
        // * Not yet decided, or decided in such a way that the literal yields true
        let can_watch = |solvable_lit: Literal| {
            !self.watched_literals.contains(&solvable_lit.solvable_id)
                && solvable_lit.eval(decision_map).unwrap_or(true)
        };

        match self.kind {
            Clause::InstallRoot => unreachable!(),
            Clause::Learnt(learnt_id) => learnt_clauses[learnt_id]
                .iter()
                .cloned()
                .find(|&l| can_watch(l))
                .map(|l| l.solvable_id),
            Clause::Constrains(..) | Clause::ForbidMultipleInstances(..) | Clause::Lock(..) => None,
            Clause::Requires(solvable_id, match_spec_id) => {
                // The solvable that added this clause
                let solvable_lit = Literal {
                    solvable_id,
                    negate: true,
                };
                if can_watch(solvable_lit) {
                    return Some(solvable_id);
                }

                // The available candidates
                for &candidate in &pool.match_spec_to_sorted_candidates[match_spec_id] {
                    let lit = Literal {
                        solvable_id: candidate,
                        negate: false,
                    };
                    if can_watch(lit) {
                        return Some(candidate);
                    }
                }

                // No solvable available to watch
                None
            }
        }
    }
}

/// Represents a literal in a SAT clause (i.e. either A or ¬A)
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub(crate) struct Literal {
    pub(crate) solvable_id: SolvableId,
    pub(crate) negate: bool,
}

impl Literal {
    /// Returns the value that would make the literal evaluate to true if assigned to the literal's solvable
    pub(crate) fn satisfying_value(self) -> bool {
        !self.negate
    }

    /// Evaluates the literal, or returns `None` if no value has been assigned to the solvable
    pub(crate) fn eval(self, decision_map: &DecisionMap) -> Option<bool> {
        decision_map
            .value(self.solvable_id)
            .map(|value| self.eval_inner(value))
    }

    fn eval_inner(self, solvable_value: bool) -> bool {
        if self.negate {
            !solvable_value
        } else {
            solvable_value
        }
    }
}

/// A representation of a clause that implements [`Debug`]
pub(crate) struct ClauseDebug<'pool, VS: VersionSet> {
    kind: Clause,
    pool: &'pool Pool<VS>,
}

impl<VS: VersionSet> Debug for ClauseDebug<'_, VS> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            Clause::InstallRoot => write!(f, "install root"),
            Clause::Learnt(learnt_id) => write!(f, "learnt clause {learnt_id:?}"),
            Clause::Requires(solvable_id, match_spec_id) => {
                let match_spec = self.pool.resolve_version_set(match_spec_id).to_string();
                write!(
                    f,
                    "{} requires {match_spec}",
                    self.pool.resolve_solvable_inner(solvable_id)
                )
            }
            Clause::Constrains(s1, s2, vset_id) => {
                write!(
                    f,
                    "{} excludes {} by {}",
                    self.pool.resolve_solvable_inner(s1),
                    self.pool.resolve_solvable_inner(s2),
                    self.pool.resolve_version_set(vset_id)
                )
            }
            Clause::Lock(locked, forbidden) => {
                write!(
                    f,
                    "{} is locked, so {} is forbidden",
                    self.pool.resolve_solvable_inner(locked),
                    self.pool.resolve_solvable_inner(forbidden)
                )
            }
            Clause::ForbidMultipleInstances(s1, _) => {
                let name = self.pool.resolve_solvable_inner(s1).package().record.name();
                write!(f, "only one {name} allowed")
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::arena::ArenaId;

    fn clause(next_clauses: [ClauseId; 2], watched_solvables: [SolvableId; 2]) -> ClauseState {
        ClauseState {
            watched_literals: watched_solvables,
            next_watches: next_clauses,

            // The kind is irrelevant here
            kind: Clause::InstallRoot,
        }
    }

    #[test]
    #[allow(clippy::bool_assert_comparison)]
    fn test_literal_satisfying_value() {
        let lit = Literal {
            solvable_id: SolvableId::root(),
            negate: true,
        };
        assert_eq!(lit.satisfying_value(), false);

        let lit = Literal {
            solvable_id: SolvableId::root(),
            negate: false,
        };
        assert_eq!(lit.satisfying_value(), true);
    }

    #[test]
    fn test_literal_eval() {
        let mut decision_map = DecisionMap::new(10);

        let literal = Literal {
            solvable_id: SolvableId::root(),
            negate: false,
        };
        let negated_literal = Literal {
            solvable_id: SolvableId::root(),
            negate: true,
        };

        // Undecided
        assert_eq!(literal.eval(&decision_map), None);
        assert_eq!(negated_literal.eval(&decision_map), None);

        // Decided
        decision_map.set(SolvableId::root(), true, 1);
        assert_eq!(literal.eval(&decision_map), Some(true));
        assert_eq!(negated_literal.eval(&decision_map), Some(false));

        decision_map.set(SolvableId::root(), false, 1);
        assert_eq!(literal.eval(&decision_map), Some(false));
        assert_eq!(negated_literal.eval(&decision_map), Some(true));
    }

    #[test]
    fn test_unlink_clause_different() {
        let clause1 = clause(
            [ClauseId::new(2), ClauseId::new(3)],
            [SolvableId::from_usize(1596), SolvableId::from_usize(1211)],
        );
        let clause2 = clause(
            [ClauseId::null(), ClauseId::new(3)],
            [SolvableId::from_usize(1596), SolvableId::from_usize(1208)],
        );
        let clause3 = clause(
            [ClauseId::null(), ClauseId::null()],
            [SolvableId::from_usize(1211), SolvableId::from_usize(42)],
        );

        // Unlink 0
        {
            let mut clause1 = clause1.clone();
            clause1.unlink_clause(&clause2, SolvableId::from_usize(1596), 0);
            assert_eq!(
                clause1.watched_literals,
                [SolvableId::from_usize(1596), SolvableId::from_usize(1211)]
            );
            assert_eq!(clause1.next_watches, [ClauseId::null(), ClauseId::new(3)])
        }

        // Unlink 1
        {
            let mut clause1 = clause1;
            clause1.unlink_clause(&clause3, SolvableId::from_usize(1211), 0);
            assert_eq!(
                clause1.watched_literals,
                [SolvableId::from_usize(1596), SolvableId::from_usize(1211)]
            );
            assert_eq!(clause1.next_watches, [ClauseId::new(2), ClauseId::null()])
        }
    }

    #[test]
    fn test_unlink_clause_same() {
        let clause1 = clause(
            [ClauseId::new(2), ClauseId::new(2)],
            [SolvableId::from_usize(1596), SolvableId::from_usize(1211)],
        );
        let clause2 = clause(
            [ClauseId::null(), ClauseId::null()],
            [SolvableId::from_usize(1596), SolvableId::from_usize(1211)],
        );

        // Unlink 0
        {
            let mut clause1 = clause1.clone();
            clause1.unlink_clause(&clause2, SolvableId::from_usize(1596), 0);
            assert_eq!(
                clause1.watched_literals,
                [SolvableId::from_usize(1596), SolvableId::from_usize(1211)]
            );
            assert_eq!(clause1.next_watches, [ClauseId::null(), ClauseId::new(2)])
        }

        // Unlink 1
        {
            let mut clause1 = clause1;
            clause1.unlink_clause(&clause2, SolvableId::from_usize(1211), 1);
            assert_eq!(
                clause1.watched_literals,
                [SolvableId::from_usize(1596), SolvableId::from_usize(1211)]
            );
            assert_eq!(clause1.next_watches, [ClauseId::new(2), ClauseId::null()])
        }
    }

    #[test]
    fn test_clause_size() {
        // This test is here to ensure we don't increase the size of `ClauseState` by accident, as
        // we are creating thousands of instances. Note: libsolv manages to bring down the size to
        // 24, so there is probably room for improvement.
        assert_eq!(std::mem::size_of::<ClauseState>(), 32);
    }
}
