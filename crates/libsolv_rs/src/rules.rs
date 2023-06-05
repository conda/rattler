use crate::decision_map::DecisionMap;
use crate::pool::{MatchSpecId, Pool};
use crate::solvable::SolvableId;
use crate::solver::RuleId;

#[derive(Clone)]
pub(crate) struct Rule {
    pub watched_literals: [SolvableId; 2],
    next_watches: [RuleId; 2],
    pub(crate) kind: RuleKind,
}

impl Rule {
    pub fn new(kind: RuleKind, learnt_rules: &[Vec<Literal>], pool: &Pool) -> Self {
        let watched_literals = kind
            .initial_watches(learnt_rules, pool)
            .unwrap_or([SolvableId::null(), SolvableId::null()]);

        let rule = Self {
            watched_literals,
            next_watches: [RuleId::null(), RuleId::null()],
            kind,
        };

        debug_assert!(!rule.has_watches() || watched_literals[0] != watched_literals[1]);

        rule
    }

    pub fn debug(&self, pool: &Pool) {
        match self.kind {
            RuleKind::InstallRoot => println!("install root"),
            RuleKind::Learnt(index) => println!("learnt rule {index}"),
            RuleKind::Requires(solvable_id, match_spec_id) => {
                let match_spec = pool.resolve_match_spec(match_spec_id).to_string();
                println!(
                    "{} requires {match_spec}",
                    pool.resolve_solvable_inner(solvable_id).display()
                )
            }
            RuleKind::Constrains(s1, s2) => {
                println!(
                    "{} excludes {}",
                    pool.resolve_solvable_inner(s1).display(),
                    pool.resolve_solvable_inner(s2).display()
                )
            }
            RuleKind::Forbids(s1, _) => {
                let name = pool
                    .resolve_solvable_inner(s1)
                    .package()
                    .record
                    .name
                    .as_str();
                println!("only one {name} allowed")
            }
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
        linked_rule: &Rule,
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
            RuleKind::InstallRoot => unreachable!(),
            RuleKind::Learnt(index) => {
                // TODO: this is probably not going to cut it for performance
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
            RuleKind::Forbids(_, _) => literals(false, false),
            RuleKind::Constrains(_, _) => literals(false, false),
            RuleKind::Requires(solvable_id, _) => {
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
            RuleKind::InstallRoot => unreachable!(),
            RuleKind::Learnt(index) => learnt_rules[index]
                .iter()
                .cloned()
                .find(|&l| can_watch(l))
                .map(|l| l.solvable_id),
            RuleKind::Forbids(_, _) => None,
            RuleKind::Constrains(_, _) => None,
            RuleKind::Requires(solvable_id, match_spec_id) => {
                // The solvable that added this rule
                let solvable_lit = Literal {
                    solvable_id,
                    negate: true,
                };
                if can_watch(solvable_lit) {
                    return Some(solvable_id);
                }

                // The available candidates
                for &candidate in pool.match_spec_to_candidates[match_spec_id.index()]
                    .as_deref()
                    .unwrap()
                {
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

    /// Returns the list of literals that constitute this rule
    pub fn literals(&self, learnt_rules: &[Vec<Literal>], pool: &Pool) -> Vec<Literal> {
        match self.kind {
            RuleKind::InstallRoot => unreachable!(),
            RuleKind::Learnt(index) => learnt_rules[index].clone(),
            RuleKind::Requires(solvable_id, match_spec_id) => {
                // All variables contribute to the conflict
                std::iter::once(Literal {
                    solvable_id,
                    negate: true,
                })
                .chain(
                    pool.match_spec_to_candidates[match_spec_id.index()]
                        .as_deref()
                        .unwrap()
                        .iter()
                        .cloned()
                        .map(|solvable_id| Literal {
                            solvable_id,
                            negate: false,
                        }),
                )
                .collect()
            }
            RuleKind::Forbids(s1, s2) => {
                vec![
                    Literal {
                        solvable_id: s1,
                        negate: true,
                    },
                    Literal {
                        solvable_id: s2,
                        negate: true,
                    },
                ]
            }
            RuleKind::Constrains(s1, s2) => {
                vec![
                    Literal {
                        solvable_id: s1,
                        negate: true,
                    },
                    Literal {
                        solvable_id: s2,
                        negate: true,
                    },
                ]
            }
        }
    }

    /// Returns the list of variables that imply that the provided solvable should be decided
    pub fn conflict_causes(
        &self,
        variable: SolvableId,
        learnt_rules: &[Vec<Literal>],
        pool: &Pool,
    ) -> Vec<Literal> {
        match self.kind {
            RuleKind::InstallRoot => unreachable!(),
            RuleKind::Learnt(index) => learnt_rules[index]
                .iter()
                .cloned()
                .filter(|lit| lit.solvable_id != variable)
                .collect(),
            RuleKind::Requires(solvable_id, match_spec_id) => {
                // All variables contribute to the conflict
                std::iter::once(Literal {
                    solvable_id,
                    negate: true,
                })
                .chain(
                    pool.match_spec_to_candidates[match_spec_id.index()]
                        .as_deref()
                        .unwrap()
                        .iter()
                        .cloned()
                        .map(|solvable_id| Literal {
                            solvable_id,
                            negate: false,
                        }),
                )
                .filter(|&l| variable != l.solvable_id)
                .collect()
            }
            RuleKind::Forbids(s1, s2) => {
                let cause = if variable == s1 { s2 } else { s1 };

                vec![Literal {
                    solvable_id: cause,
                    negate: true,
                }]
            }

            RuleKind::Constrains(s1, s2) => {
                let cause = if variable == s1 { s2 } else { s1 };

                vec![Literal {
                    solvable_id: cause,
                    negate: true,
                }]
            }
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub(crate) struct Literal {
    pub(crate) solvable_id: SolvableId,
    pub(crate) negate: bool,
}

impl Literal {
    pub(crate) fn satisfying_value(self) -> bool {
        !self.negate
    }

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

#[derive(Copy, Clone, Debug)]
pub(crate) enum RuleKind {
    InstallRoot,
    /// The solvable requires the candidates associated to the match spec
    ///
    /// In SAT terms: (¬A ∨ B1 ∨ B2 ∨ ... ∨ B99), where B1 to B99 represent the possible candidates
    /// for the provided match spec.
    Requires(SolvableId, MatchSpecId),
    /// The left solvable forbids installing the right solvable
    ///
    /// Used to ensure only a single version of a package is installed
    ///
    /// In SAT terms: (¬A ∨ ¬B)
    Forbids(SolvableId, SolvableId),
    /// Similar to forbid, but created due to a constrains relationship
    Constrains(SolvableId, SolvableId),
    /// Learnt rule
    Learnt(usize),
}

impl RuleKind {
    fn initial_watches(
        &self,
        learnt_rules: &[Vec<Literal>],
        pool: &Pool,
    ) -> Option<[SolvableId; 2]> {
        match self {
            RuleKind::InstallRoot => None,
            RuleKind::Constrains(s1, s2) => Some([*s1, *s2]),
            RuleKind::Forbids(s1, s2) => Some([*s1, *s2]),
            RuleKind::Learnt(index) => {
                let literals = &learnt_rules[*index];
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
            RuleKind::Requires(id, match_spec) => {
                let candidates = pool.match_spec_to_candidates[match_spec.index()]
                    .as_ref()
                    .unwrap();

                if candidates.is_empty() {
                    None
                } else {
                    Some([*id, candidates[0]])
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn rule(next_rules: [RuleId; 2], watched_solvables: [SolvableId; 2]) -> Rule {
        Rule {
            watched_literals: watched_solvables,
            next_watches: next_rules,

            // The kind is irrelevant here
            kind: RuleKind::InstallRoot,
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
}
