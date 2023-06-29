use crate::id::RuleId;
use crate::id::SolvableId;
use crate::mapping::Mapping;
use crate::solver::rule::RuleState;

/// A map from solvables to the rules that are watching them
pub(crate) struct WatchMap {
    /// Note: the map is to a single rule, but rules form a linked list, so it is possible to go
    /// from one to the next
    map: Mapping<SolvableId, RuleId>,
}

impl WatchMap {
    pub(crate) fn new() -> Self {
        Self {
            map: Mapping::empty(),
        }
    }

    pub(crate) fn initialize(&mut self, solvable_count: usize) {
        self.map = Mapping::new(vec![RuleId::null(); solvable_count]);
    }

    pub(crate) fn start_watching(&mut self, rule: &mut RuleState, rule_id: RuleId) {
        for (watch_index, watched_solvable) in rule.watched_literals.into_iter().enumerate() {
            let already_watching = self.first_rule_watching_solvable(watched_solvable);
            rule.link_to_rule(watch_index, already_watching);
            self.watch_solvable(watched_solvable, rule_id);
        }
    }

    pub(crate) fn update_watched(
        &mut self,
        predecessor_rule: Option<&mut RuleState>,
        rule: &mut RuleState,
        rule_id: RuleId,
        watch_index: usize,
        previous_watch: SolvableId,
        new_watch: SolvableId,
    ) {
        // Remove this rule from its current place in the linked list, because we
        // are no longer watching what brought us here
        if let Some(predecessor_rule) = predecessor_rule {
            // Unlink the rule
            predecessor_rule.unlink_rule(rule, previous_watch, watch_index);
        } else {
            // This was the first rule in the chain
            self.map[previous_watch] = rule.get_linked_rule(watch_index);
        }

        // Set the new watch
        rule.watched_literals[watch_index] = new_watch;
        rule.link_to_rule(watch_index, self.map[new_watch]);
        self.map[new_watch] = rule_id;
    }

    pub(crate) fn first_rule_watching_solvable(&mut self, watched_solvable: SolvableId) -> RuleId {
        self.map[watched_solvable]
    }

    pub(crate) fn watch_solvable(&mut self, watched_solvable: SolvableId, id: RuleId) {
        self.map[watched_solvable] = id;
    }
}
