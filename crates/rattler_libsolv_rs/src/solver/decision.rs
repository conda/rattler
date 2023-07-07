use crate::id::{ClauseId, SolvableId};

/// Represents an assignment to a variable
#[derive(Copy, Clone, Eq, PartialEq)]
pub(crate) struct Decision {
    pub(crate) solvable_id: SolvableId,
    pub(crate) value: bool,
    pub(crate) derived_from: ClauseId,
}

impl Decision {
    pub(crate) fn new(solvable: SolvableId, value: bool, derived_from: ClauseId) -> Self {
        Self {
            solvable_id: solvable,
            value,
            derived_from,
        }
    }
}
