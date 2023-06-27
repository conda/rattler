use crate::id::SolvableId;
use rattler_conda_types::MatchSpec;

#[derive(Default)]
pub struct SolveJobs {
    pub(crate) install: Vec<MatchSpec>,
    pub(crate) favor: Vec<SolvableId>,
    pub(crate) lock: Vec<SolvableId>,
}

impl SolveJobs {
    /// The specified spec must be installed
    pub fn install(&mut self, match_spec: MatchSpec) {
        self.install.push(match_spec);
    }

    /// Favor the specified solvable over other variants. This doesnt mean this variant will be
    /// used. To guarantee a solvable is used (if selected) use the `Self::lock` function.
    pub fn favor(&mut self, id: SolvableId) {
        self.favor.push(id);
    }

    /// Lock the specified solvable over other variants. This implies that not other variant will
    /// ever be considered.
    pub fn lock(&mut self, id: SolvableId) {
        self.lock.push(id);
    }
}
