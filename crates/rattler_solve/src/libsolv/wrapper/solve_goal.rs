use super::ffi;
use crate::libsolv::wrapper::ffi::{
    SOLVER_DISFAVOR, SOLVER_ERASE, SOLVER_FAVOR, SOLVER_INSTALL, SOLVER_LOCK, SOLVER_SOLVABLE,
    SOLVER_SOLVABLE_PROVIDES, SOLVER_UPDATE, SOLVER_WEAK,
};
use crate::libsolv::wrapper::pool::MatchSpecId;
use crate::libsolv::wrapper::solvable::SolvableId;
use std::os::raw::c_int;

/// Wrapper for libsolv queue type that stores jobs. This type provides a safe wrapper around a goal
/// state for libsolv.
pub struct SolveGoal {
    queue: ffi::Queue,
}

impl Default for SolveGoal {
    fn default() -> Self {
        // Safe because we know for a fact that the queue exists
        unsafe {
            // Create a queue pointer and initialize it
            let mut queue = ffi::Queue {
                elements: std::ptr::null_mut(),
                count: 0,
                alloc: std::ptr::null_mut(),
                left: 0,
            };
            // This initializes some internal libsolv stuff
            ffi::queue_init(&mut queue as *mut ffi::Queue);
            Self { queue }
        }
    }
}

/// This drop implementation drops the internal libsolv queue
impl Drop for SolveGoal {
    fn drop(&mut self) {
        // Safe because this pointer exists
        unsafe {
            ffi::queue_free(self.as_inner_mut());
        }
    }
}

impl SolveGoal {
    /// Returns the ffi::Queue as a mutable pointer
    pub fn as_inner_mut(&mut self) -> *mut ffi::Queue {
        &mut self.queue as *mut ffi::Queue
    }

    /// Returns the ffi::Queue as a const pointer
    pub fn as_inner_ptr(&self) -> *const ffi::Queue {
        &self.queue as *const ffi::Queue
    }
}

impl SolveGoal {
    /// The specified spec must be installed
    pub fn install(&mut self, match_spec: MatchSpecId, optional: bool) {
        let action = if optional {
            SOLVER_INSTALL | SOLVER_WEAK
        } else {
            SOLVER_INSTALL
        };
        self.push_id_with_flags(match_spec, action | SOLVER_SOLVABLE_PROVIDES);
    }

    /// The specified spec must not be installed.
    pub fn erase(&mut self, match_spec: MatchSpecId) {
        self.push_id_with_flags(match_spec, SOLVER_ERASE | SOLVER_SOLVABLE_PROVIDES);
    }

    /// The highest possible spec must be installed
    pub fn update(&mut self, match_spec: MatchSpecId) {
        self.push_id_with_flags(match_spec, SOLVER_UPDATE | SOLVER_SOLVABLE_PROVIDES);
    }

    /// Favor the specified solvable over other variants. This doesnt mean this variant will be
    /// used. To guarantee a solvable is used (if selected) use the `Self::lock` function.
    pub fn favor(&mut self, solvable: SolvableId) {
        self.push_id_with_flags(solvable, SOLVER_SOLVABLE | SOLVER_FAVOR);
    }

    /// Lock the specified solvable over other variants. This implies that not other variant will
    /// ever be considered.
    pub fn lock(&mut self, solvable: SolvableId) {
        self.push_id_with_flags(solvable, SOLVER_SOLVABLE | SOLVER_LOCK);
    }

    /// Disfavor the specified variant over other variants. This does not mean it will never be
    /// selected, but other variants are considered first.
    pub fn disfavor(&mut self, solvable: SolvableId) {
        self.push_id_with_flags(solvable, SOLVER_SOLVABLE | SOLVER_DISFAVOR);
    }

    /// Push and id and flag into the queue
    fn push_id_with_flags(&mut self, id: impl Into<ffi::Id>, flags: u32) {
        unsafe {
            ffi::queue_insert2(
                self.as_inner_mut(),
                self.queue.count,
                flags as c_int,
                id.into(),
            );
        }
    }
}
