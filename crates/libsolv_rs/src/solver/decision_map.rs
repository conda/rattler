use crate::id::SolvableId;
use crate::mapping::Mapping;
use std::cmp::Ordering;

/// Represents a decision (i.e. an assignment to a solvable) and the level at which it was made
///
/// = 0: undecided
/// > 0: level of decision when the solvable is set to true
/// < 0: level of decision when the solvable is set to false
#[repr(transparent)]
#[derive(Copy, Clone)]
struct DecisionAndLevel(i64);

impl DecisionAndLevel {
    fn undecided() -> DecisionAndLevel {
        DecisionAndLevel(0)
    }

    fn set(&mut self, value: bool, level: u32) {
        self.0 = if value { level as i64 } else { -(level as i64) };
    }

    fn value(self) -> Option<bool> {
        match self.0.cmp(&0) {
            Ordering::Less => Some(false),
            Ordering::Equal => None,
            Ordering::Greater => Some(true),
        }
    }

    fn level(self) -> u32 {
        self.0.unsigned_abs() as u32
    }
}

/// A map of the assignments to all solvables
pub(crate) struct DecisionMap {
    map: Mapping<SolvableId, DecisionAndLevel>,
}

impl DecisionMap {
    pub(crate) fn new(solvable_count: u32) -> Self {
        Self {
            map: Mapping::new(vec![DecisionAndLevel::undecided(); solvable_count as usize]),
        }
    }

    pub(crate) fn solvable_count(&self) -> u32 {
        self.map.len() as u32
    }

    pub(crate) fn reset(&mut self, solvable_id: SolvableId) {
        self.map[solvable_id] = DecisionAndLevel::undecided();
    }

    pub(crate) fn set(&mut self, solvable_id: SolvableId, value: bool, level: u32) {
        self.map[solvable_id].set(value, level);
    }

    pub(crate) fn level(&self, solvable_id: SolvableId) -> u32 {
        self.map[solvable_id].level()
    }

    pub(crate) fn value(&self, solvable_id: SolvableId) -> Option<bool> {
        self.map[solvable_id].value()
    }
}
