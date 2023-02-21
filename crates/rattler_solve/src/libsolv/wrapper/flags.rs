use super::ffi::{
    SOLVER_ERASE, SOLVER_FLAG_ALLOW_DOWNGRADE, SOLVER_FLAG_ALLOW_UNINSTALL, SOLVER_INSTALL,
    SOLVER_SOLVABLE_PROVIDES, SOLVER_UPDATE,
};
use crate::RequestedAction;

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

#[repr(transparent)]
pub struct SolvableFlags(u32);

impl From<RequestedAction> for SolvableFlags {
    fn from(action: RequestedAction) -> Self {
        let flag = match action {
            RequestedAction::Install => SOLVER_INSTALL,
            RequestedAction::Remove => SOLVER_ERASE,
            RequestedAction::Update => SOLVER_UPDATE,
        };

        SolvableFlags(flag)
    }
}

impl SolvableFlags {
    pub fn inner(self) -> i32 {
        (self.0 | SOLVER_SOLVABLE_PROVIDES) as i32
    }
}
