use crate::libsolv::ffi;
use crate::libsolv::pool::Pool;
use std::ptr::NonNull;

/// Solvable in libsolv
pub struct Solvable(NonNull<ffi::Solvable>);

/// Represents a solvable in a [`Repo`] or [`Pool`]
pub struct SolvableId(pub(super) ffi::Id);

impl From<SolvableId> for ffi::Id {
    fn from(s: SolvableId) -> Self {
        s.0
    }
}

impl SolvableId {
    /// Resolve to the interned type returns a Solvable
    fn resolve(&self, pool: &Pool) -> Solvable {
        // Safe because the new-type wraps the ffi::id and cant be created otherwise
        unsafe {
            // Re-implement pool_id2solvable, as it's a static inline function, we can't use it :(
            let solvable = (*pool.0.as_ptr()).solvables;
            // Apparently the solvable is offset by the id from the first solvable
            let solvable = solvable.offset(self.0 as isize);
            Solvable(NonNull::new(solvable).expect("solvable cannot be null"))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::libsolv::pool::Pool;

    #[test]
    pub fn test_solvable_creation() {
        let mut pool = Pool::default();
        let mut repo = pool.create_repo("bla");
        let solvable_id = repo.add_solvable();
        let _solvable = solvable_id.resolve(&pool);
    }
}
