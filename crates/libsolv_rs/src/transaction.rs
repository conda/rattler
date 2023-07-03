use crate::id::SolvableId;

/// The transaction that results from the jobs provided to the solver and the found solution
pub struct Transaction {
    /// The solvables that should be installed
    pub steps: Vec<SolvableId>,
}
