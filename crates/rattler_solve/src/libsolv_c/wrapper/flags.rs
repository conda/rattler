use super::ffi::{SOLVER_FLAG_ALLOW_DOWNGRADE, SOLVER_FLAG_ALLOW_UNINSTALL};

#[repr(transparent)]
pub struct SolverFlag(u32);

impl SolverFlag {
    pub fn allow_uninstall() -> SolverFlag {
        SolverFlag(SOLVER_FLAG_ALLOW_UNINSTALL)
    }

    pub fn allow_downgrade() -> SolverFlag {
        SolverFlag(SOLVER_FLAG_ALLOW_DOWNGRADE)
    }

    pub fn inner(self) -> i32 {
        self.0 as i32
    }
}
