use crate::id::SolvableId;
use std::fmt::{Display, Formatter};

/// Represents the operations that should be performed to achieve the desired state, based on the
/// jobs provided to the solver and their solution
pub struct Transaction {
    pub steps: Vec<(SolvableId, TransactionKind)>,
}

#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum TransactionKind {
    Install,
}

impl Display for TransactionKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}
