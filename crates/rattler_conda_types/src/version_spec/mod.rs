//! Compatibility re-exports for Conda version constraints.
//!
//! Prefer importing these types from [`rattler_conda_version`]. They remain
//! available here so existing `rattler_conda_types` users do not need to change
//! their imports.

pub(crate) mod version_tree;

/// Returns whether `character` can begin a Conda version constraint.
pub(crate) fn is_start_of_version_constraint(character: char) -> bool {
    matches!(character, '>' | '<' | '=' | '!' | '~')
}

pub use rattler_conda_version::{
    VersionSpec,
    version_spec::{
        EqualityOperator, LogicalOperator, ParseConstraintError, ParseVersionSpecError,
        RangeOperator, StrictRangeOperator, VersionOperators,
    },
};
