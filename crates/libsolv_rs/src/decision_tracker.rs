use crate::decision_map::DecisionMap;
use crate::solvable::SolvableId;
use crate::solver::Decision;

pub(crate) struct DecisionTracker {
    map: DecisionMap,
    stack: Vec<Decision>,
    propagate_index: usize,
}

impl DecisionTracker {
    pub(crate) fn new(nsolvables: u32) -> Self {
        Self {
            map: DecisionMap::new(nsolvables),
            stack: Vec::new(),
            propagate_index: 0,
        }
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::new(self.map.nsolvables());
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    pub(crate) fn assigned_value(&self, solvable_id: SolvableId) -> Option<bool> {
        self.map.value(solvable_id)
    }

    pub(crate) fn map(&self) -> &DecisionMap {
        &self.map
    }

    pub(crate) fn stack(&self) -> &[Decision] {
        &self.stack
    }

    pub(crate) fn level(&self, solvable_id: SolvableId) -> u32 {
        self.map.level(solvable_id)
    }

    /// Attempts to add a decision
    ///
    /// Returns true if the solvable was undecided, false if it was already decided to the same value
    ///
    /// Returns an error if the solvable was decided to a different value (which means there is a conflict)
    pub(crate) fn try_add_decision(&mut self, decision: Decision, level: u32) -> Result<bool, ()> {
        match self.map.value(decision.solvable_id) {
            None => {
                self.map.set(decision.solvable_id, decision.value, level);
                self.stack.push(decision);
                Ok(true)
            }
            Some(value) if value == decision.value => Ok(false),
            _ => Err(()),
        }
    }

    pub(crate) fn undo_until(&mut self, level: u32) {
        while let Some(decision) = self.stack.last() {
            if self.level(decision.solvable_id) <= level {
                break;
            }

            self.undo_last();
        }
    }

    pub(crate) fn undo_last(&mut self) -> (Decision, u32) {
        let decision = self.stack.pop().unwrap();
        self.map.reset(decision.solvable_id);

        self.propagate_index = self.stack.len();

        let top_decision = self.stack.last().unwrap();
        (decision, self.map.level(top_decision.solvable_id))
    }

    pub(crate) fn next_unpropagated(&mut self) -> Option<Decision> {
        let &decision = self.stack[self.propagate_index..].iter().next()?;
        self.propagate_index += 1;
        Some(decision)
    }
}
