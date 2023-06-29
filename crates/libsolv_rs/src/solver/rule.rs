use crate::id::MatchSpecId;
use crate::id::RuleId;
use crate::id::SolvableId;
use crate::mapping::Mapping;
use crate::pool::Pool;
use crate::solver::decision_map::DecisionMap;
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
/// of rules for a particular dependency resolution problem, and we try to keep the [`Rule`] enum
/// small. A naive implementation would store a `Vec<Literal>`.
#[derive(Copy, Clone, Debug)]
pub(crate) enum Rule {
    /// An assertion that the root solvable must be installed
    ///
    /// In SAT terms: (root)
    InstallRoot,
    /// The solvable requires the candidates associated to the match spec
    ///
    /// In SAT terms: (¬A ∨ B1 ∨ B2 ∨ ... ∨ B99), where B1 to B99 represent the possible candidates
    /// for the provided match spec
    Requires(SolvableId, MatchSpecId),
    /// Ensures only a single version of a package is installed
    ///
    /// Usage: generate one [`Rule::ForbidMultipleInstances`] rule for each possible combination of
    /// packages under the same name. The rule itself forbids two solvables from being installed at
    /// the same time.
    ///
    /// In SAT terms: (¬A ∨ ¬B)
    ForbidMultipleInstances(SolvableId, SolvableId),
    /// Forbids packages that do not satisfy a solvable's constrains
    ///
    /// Usage: for each constrains relationship in a package, determine all the candidates that do
    /// _not_ satisfy it, and create one [`Rule::Constrains`]. The rule itself forbids two solvables
    /// from being installed at the same time, just as [`Rule::ForbidMultipleInstances`], but it
    /// pays off to have a separate variant for user-friendly error messages.
    ///
    /// In SAT terms: (¬A ∨ ¬B)
    Constrains(SolvableId, SolvableId),
    /// Forbids the package on the right-hand side
    ///
    /// Note that the package on the left-hand side is not part of the rule, but just context to
    /// know which exact package was locked (necessary for user-friendly error messages)
    ///
    /// In SAT terms: (¬root ∨ ¬B). Note that we could encode this as an assertion (¬B), but that
    /// would require additional logic in the solver.
    Lock(SolvableId, SolvableId),
    /// A rule learnt during solving
    ///
    /// The `usize` is an index that can be used to retrieve the rule's literals, which are stored
    /// elsewhere to prevent the size of [`Rule`] from blowing up
    Learnt(usize),
}

impl Rule {
    /// Returns the ids of the solvables that will be watched right after the rule is created
    fn initial_watches(
        &self,
        learnt_rules: &[Vec<Literal>],
        match_spec_to_candidates: &Mapping<MatchSpecId, Vec<SolvableId>>,
    ) -> Option<[SolvableId; 2]> {
        match self {
            Rule::InstallRoot => None,
            Rule::Constrains(s1, s2) | Rule::ForbidMultipleInstances(s1, s2) => Some([*s1, *s2]),
            Rule::Lock(_, s) => Some([SolvableId::root(), *s]),
            &Rule::Learnt(index) => {
                let literals = &learnt_rules[index];
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
            &Rule::Requires(id, match_spec) => {
                let candidates = &match_spec_to_candidates[match_spec];
                if candidates.is_empty() {
                    None
                } else {
                    Some([id, candidates[0]])
                }
            }
        }
    }

    /// Visits each literal in the rule
    pub fn visit_literals(
        &self,
        learnt_rules: &[Vec<Literal>],
        pool: &Pool,
        mut visit: impl FnMut(Literal) -> bool,
    ) {
        match *self {
            Rule::InstallRoot => unreachable!(),
            Rule::Learnt(index) => {
                for &literal in &learnt_rules[index] {
                    if !visit(literal) {
                        return;
                    }
                }
            }
            Rule::Requires(solvable_id, match_spec_id) => {
                if !visit(Literal {
                    solvable_id,
                    negate: true,
                }) {
                    return;
                }

                for &solvable_id in &pool.match_spec_to_candidates[match_spec_id] {
                    if !visit(Literal {
                        solvable_id,
                        negate: false,
                    }) {
                        return;
                    }
                }
            }
            Rule::Constrains(s1, s2) | Rule::ForbidMultipleInstances(s1, s2) => {
                if !visit(Literal {
                    solvable_id: s1,
                    negate: true,
                }) {
                    return;
                }

                visit(Literal {
                    solvable_id: s2,
                    negate: true,
                });
            }
            Rule::Lock(_, s) => {
                if !visit(Literal {
                    solvable_id: SolvableId::root(),
                    negate: true,
                }) {
                    return;
                }

                visit(Literal {
                    solvable_id: s,
                    negate: true,
                });
            }
        }
    }
}

/// Keeps track of the literals watched by a [`Rule`] and the state associated to two linked lists
/// this rule is part of
///
/// In our SAT implementation, each rule tracks two literals present in its clause, to be notified
/// when the value assigned to the variable has changed (this technique is known as _watches_).
/// Rules that are tracking the same variable are grouped together in a linked list, so it becomes
/// easy to notify them all.
#[derive(Clone)]
pub(crate) struct RuleState {
    // The ids of the solvables this rule is watching
    pub watched_literals: [SolvableId; 2],
    // The ids of the next rule in each linked list that this rule is part of
    next_watches: [RuleId; 2],
    // The clause itself
    pub(crate) kind: Rule,
}

impl RuleState {
    pub fn new(
        kind: Rule,
        learnt_rules: &[Vec<Literal>],
        match_spec_to_candidates: &Mapping<MatchSpecId, Vec<SolvableId>>,
    ) -> Self {
        let watched_literals = kind
            .initial_watches(learnt_rules, match_spec_to_candidates)
            .unwrap_or([SolvableId::null(), SolvableId::null()]);

        let rule = Self {
            watched_literals,
            next_watches: [RuleId::null(), RuleId::null()],
            kind,
        };

        debug_assert!(!rule.has_watches() || watched_literals[0] != watched_literals[1]);

        rule
    }

    pub fn debug<'a>(&self, pool: &'a Pool) -> RuleDebug<'a> {
        RuleDebug {
            kind: self.kind,
            pool,
        }
    }

    pub fn link_to_rule(&mut self, watch_index: usize, linked_rule: RuleId) {
        self.next_watches[watch_index] = linked_rule;
    }

    pub fn get_linked_rule(&self, watch_index: usize) -> RuleId {
        self.next_watches[watch_index]
    }

    pub fn unlink_rule(
        &mut self,
        linked_rule: &RuleState,
        watched_solvable: SolvableId,
        linked_rule_watch_index: usize,
    ) {
        if self.watched_literals[0] == watched_solvable {
            self.next_watches[0] = linked_rule.next_watches[linked_rule_watch_index];
        } else {
            debug_assert_eq!(self.watched_literals[1], watched_solvable);
            self.next_watches[1] = linked_rule.next_watches[linked_rule_watch_index];
        }
    }

    pub fn next_watched_rule(&self, solvable_id: SolvableId) -> RuleId {
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
        learnt_rules: &[Vec<Literal>],
    ) -> Option<([Literal; 2], usize)> {
        debug_assert!(self.watched_literals.contains(&solvable_id));

        let literals @ [w1, w2] = self.watched_literals(learnt_rules);

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

    pub fn watched_literals(&self, learnt_rules: &[Vec<Literal>]) -> [Literal; 2] {
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
            Rule::InstallRoot => unreachable!(),
            Rule::Learnt(index) => {
                // TODO: we might want to do something else for performance, like keeping the whole
                // literal in `self.watched_literals`, to avoid lookups... But first we should
                // benchmark!
                let &w1 = learnt_rules[index]
                    .iter()
                    .find(|l| l.solvable_id == self.watched_literals[0])
                    .unwrap();
                let &w2 = learnt_rules[index]
                    .iter()
                    .find(|l| l.solvable_id == self.watched_literals[1])
                    .unwrap();
                [w1, w2]
            }
            Rule::Constrains(..) | Rule::ForbidMultipleInstances(..) | Rule::Lock(..) => {
                literals(false, false)
            }
            Rule::Requires(solvable_id, _) => {
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

    pub fn next_unwatched_variable(
        &self,
        pool: &Pool,
        learnt_rules: &[Vec<Literal>],
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
            Rule::InstallRoot => unreachable!(),
            Rule::Learnt(index) => learnt_rules[index]
                .iter()
                .cloned()
                .find(|&l| can_watch(l))
                .map(|l| l.solvable_id),
            Rule::Constrains(..) | Rule::ForbidMultipleInstances(..) | Rule::Lock(..) => None,
            Rule::Requires(solvable_id, match_spec_id) => {
                // The solvable that added this rule
                let solvable_lit = Literal {
                    solvable_id,
                    negate: true,
                };
                if can_watch(solvable_lit) {
                    return Some(solvable_id);
                }

                // The available candidates
                for &candidate in &pool.match_spec_to_candidates[match_spec_id] {
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

/// A representation of a rule that implements [`Debug`]
pub(crate) struct RuleDebug<'a> {
    kind: Rule,
    pool: &'a Pool<'a>,
}

impl Debug for RuleDebug<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            Rule::InstallRoot => write!(f, "install root"),
            Rule::Learnt(index) => write!(f, "learnt rule {index}"),
            Rule::Requires(solvable_id, match_spec_id) => {
                let match_spec = self.pool.resolve_match_spec(match_spec_id).to_string();
                write!(
                    f,
                    "{} requires {match_spec}",
                    self.pool.resolve_solvable_inner(solvable_id).display()
                )
            }
            Rule::Constrains(s1, s2) => {
                write!(
                    f,
                    "{} excludes {}",
                    self.pool.resolve_solvable_inner(s1).display(),
                    self.pool.resolve_solvable_inner(s2).display()
                )
            }
            Rule::Lock(locked, forbidden) => {
                write!(
                    f,
                    "{} is locked, so {} is forbidden",
                    self.pool.resolve_solvable_inner(locked).display(),
                    self.pool.resolve_solvable_inner(forbidden).display()
                )
            }
            Rule::ForbidMultipleInstances(s1, _) => {
                let name = self
                    .pool
                    .resolve_solvable_inner(s1)
                    .package()
                    .record
                    .name
                    .as_str();
                write!(f, "only one {name} allowed")
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn rule(next_rules: [RuleId; 2], watched_solvables: [SolvableId; 2]) -> RuleState {
        RuleState {
            watched_literals: watched_solvables,
            next_watches: next_rules,

            // The kind is irrelevant here
            kind: Rule::InstallRoot,
        }
    }

    #[test]
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
    fn test_unlink_rule_different() {
        let rule1 = rule(
            [RuleId::new(2), RuleId::new(3)],
            [SolvableId::new(1596), SolvableId::new(1211)],
        );
        let rule2 = rule(
            [RuleId::null(), RuleId::new(3)],
            [SolvableId::new(1596), SolvableId::new(1208)],
        );
        let rule3 = rule(
            [RuleId::null(), RuleId::null()],
            [SolvableId::new(1211), SolvableId::new(42)],
        );

        // Unlink 0
        {
            let mut rule1 = rule1.clone();
            rule1.unlink_rule(&rule2, SolvableId::new(1596), 0);
            assert_eq!(
                rule1.watched_literals,
                [SolvableId::new(1596), SolvableId::new(1211)]
            );
            assert_eq!(rule1.next_watches, [RuleId::null(), RuleId::new(3)])
        }

        // Unlink 1
        {
            let mut rule1 = rule1.clone();
            rule1.unlink_rule(&rule3, SolvableId::new(1211), 0);
            assert_eq!(
                rule1.watched_literals,
                [SolvableId::new(1596), SolvableId::new(1211)]
            );
            assert_eq!(rule1.next_watches, [RuleId::new(2), RuleId::null()])
        }
    }

    #[test]
    fn test_unlink_rule_same() {
        let rule1 = rule(
            [RuleId::new(2), RuleId::new(2)],
            [SolvableId::new(1596), SolvableId::new(1211)],
        );
        let rule2 = rule(
            [RuleId::null(), RuleId::null()],
            [SolvableId::new(1596), SolvableId::new(1211)],
        );

        // Unlink 0
        {
            let mut rule1 = rule1.clone();
            rule1.unlink_rule(&rule2, SolvableId::new(1596), 0);
            assert_eq!(
                rule1.watched_literals,
                [SolvableId::new(1596), SolvableId::new(1211)]
            );
            assert_eq!(rule1.next_watches, [RuleId::null(), RuleId::new(2)])
        }

        // Unlink 1
        {
            let mut rule1 = rule1.clone();
            rule1.unlink_rule(&rule2, SolvableId::new(1211), 1);
            assert_eq!(
                rule1.watched_literals,
                [SolvableId::new(1596), SolvableId::new(1211)]
            );
            assert_eq!(rule1.next_watches, [RuleId::new(2), RuleId::null()])
        }
    }

    #[test]
    fn test_rule_size() {
        // This test is here to ensure we don't increase the size of `Rule` by accident, as we are
        // creating thousands of instances. Note: libsolv manages to bring down the size to 24, so
        // there is probably room for improvement.
        assert_eq!(std::mem::size_of::<RuleState>(), 32);
    }
}
