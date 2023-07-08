use crate::id::ClauseId;
use crate::id::SolvableId;
use crate::mapping::Mapping;
use crate::solver::clause::ClauseState;

/// A map from solvables to the clauses that are watching them
pub(crate) struct WatchMap {
    /// Note: the map is to a single clause, but clauses form a linked list, so it is possible to go
    /// from one to the next
    map: Mapping<SolvableId, ClauseId>,
}

impl WatchMap {
    pub(crate) fn new() -> Self {
        Self {
            map: Mapping::empty(),
        }
    }

    pub(crate) fn initialize(&mut self, solvable_count: usize) {
        self.map = Mapping::new(vec![ClauseId::null(); solvable_count]);
    }

    pub(crate) fn start_watching(&mut self, clause: &mut ClauseState, clause_id: ClauseId) {
        for (watch_index, watched_solvable) in clause.watched_literals.into_iter().enumerate() {
            let already_watching = self.first_clause_watching_solvable(watched_solvable);
            clause.link_to_clause(watch_index, already_watching);
            self.watch_solvable(watched_solvable, clause_id);
        }
    }

    pub(crate) fn update_watched(
        &mut self,
        predecessor_clause: Option<&mut ClauseState>,
        clause: &mut ClauseState,
        clause_id: ClauseId,
        watch_index: usize,
        previous_watch: SolvableId,
        new_watch: SolvableId,
    ) {
        // Remove this clause from its current place in the linked list, because we
        // are no longer watching what brought us here
        if let Some(predecessor_clause) = predecessor_clause {
            // Unlink the clause
            predecessor_clause.unlink_clause(clause, previous_watch, watch_index);
        } else {
            // This was the first clause in the chain
            self.map[previous_watch] = clause.get_linked_clause(watch_index);
        }

        // Set the new watch
        clause.watched_literals[watch_index] = new_watch;
        clause.link_to_clause(watch_index, self.map[new_watch]);
        self.map[new_watch] = clause_id;
    }

    pub(crate) fn first_clause_watching_solvable(
        &mut self,
        watched_solvable: SolvableId,
    ) -> ClauseId {
        self.map[watched_solvable]
    }

    pub(crate) fn watch_solvable(&mut self, watched_solvable: SolvableId, id: ClauseId) {
        self.map[watched_solvable] = id;
    }
}
