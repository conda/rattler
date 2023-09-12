use crate::id::SolvableId;
use std::cmp::Ordering;
use std::collections::HashMap;

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
        *self = Self::with_value_and_level(value, level);
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

    fn with_value_and_level(value: bool, level: u32) -> Self {
        Self(if value { level as i64 } else { -(level as i64) })
    }
}

/// A map of the assignments to all solvables
pub(crate) struct DecisionMap {
    // TODO: (BasZ) instead of a hashmap it might be better to use a Mapping.
    map: HashMap<SolvableId, DecisionAndLevel>,
}

impl DecisionMap {
    pub(crate) fn new() -> Self {
        Self {
            map: Default::default(),
        }
    }

    pub(crate) fn reset(&mut self, solvable_id: SolvableId) {
        self.map.remove(&solvable_id);
    }

    pub(crate) fn set(&mut self, solvable_id: SolvableId, value: bool, level: u32) {
        self.map.insert(
            solvable_id,
            DecisionAndLevel::with_value_and_level(value, level),
        );
    }

    pub(crate) fn level(&self, solvable_id: SolvableId) -> u32 {
        self.map.get(&solvable_id).map_or(0, |d| d.level())
    }

    pub(crate) fn value(&self, solvable_id: SolvableId) -> Option<bool> {
        self.map.get(&solvable_id).map_or(None, |d| d.value())
    }
}
