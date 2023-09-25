use crate::internal::{arena::ArenaId, id::SolvableId};
use crate::{PackageName, Pool, VersionSet};
use std::cmp::Ordering;
use std::fmt::{Display, Formatter};

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

/// A map of the assignments to solvables.
pub(crate) struct DecisionMap {
    map: Vec<DecisionAndLevel>,
}

impl DecisionMap {
    pub fn new() -> Self {
        Self {
            map: Default::default(),
        }
    }

    pub fn reset(&mut self, solvable_id: SolvableId) {
        let solvable_id = solvable_id.to_usize();
        if solvable_id < self.map.len() {
            // SAFE: because we check that the solvable id is within bounds
            unsafe { *self.map.get_unchecked_mut(solvable_id) = DecisionAndLevel::undecided() };
        }
    }

    pub fn set(&mut self, solvable_id: SolvableId, value: bool, level: u32) {
        let solvable_id = solvable_id.to_usize();
        if solvable_id >= self.map.len() {
            self.map
                .resize_with(solvable_id + 1, DecisionAndLevel::undecided);
        }

        // SAFE: because we ensured that vec contains at least the correct number of elements.
        unsafe {
            *self.map.get_unchecked_mut(solvable_id) =
                DecisionAndLevel::with_value_and_level(value, level)
        };
    }

    pub fn level(&self, solvable_id: SolvableId) -> u32 {
        self.map
            .get(solvable_id.to_usize())
            .map_or(0, |d| d.level())
    }

    pub fn value(&self, solvable_id: SolvableId) -> Option<bool> {
        self.map.get(solvable_id.to_usize()).and_then(|d| d.value())
    }

    /// Returns an object that can be used to display the contents of the decision map in a human readable fashion.
    #[allow(unused)]
    pub fn display<'a, VS: VersionSet, N: PackageName + Display>(
        &'a self,
        pool: &'a Pool<VS, N>,
    ) -> DecisionMapDisplay<'a, VS, N> {
        DecisionMapDisplay { map: self, pool }
    }
}

pub struct DecisionMapDisplay<'a, VS: VersionSet, N: PackageName + Display> {
    map: &'a DecisionMap,
    pool: &'a Pool<VS, N>,
}

impl<'a, VS: VersionSet, N: PackageName + Display> Display for DecisionMapDisplay<'a, VS, N> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for (id, solvable) in self.pool.solvables.iter() {
            write!(f, "{} := ", solvable.display(self.pool))?;
            if let Some(value) = self.map.value(id) {
                writeln!(
                    f,
                    "{} (level: {})",
                    if value { "true " } else { "false" },
                    self.map.level(id)
                )?;
            } else {
                writeln!(f, "<undecided>")?;
            }
        }
        Ok(())
    }
}
