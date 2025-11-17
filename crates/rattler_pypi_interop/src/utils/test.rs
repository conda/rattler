//! Various utilities for testing.

/// Sets a snapshot suffix for the current scope using the `insta` crate.
///
/// This macro creates a new `insta::Settings` instance, sets the snapshot suffix
/// using the provided expressions, and binds the settings to the current scope.
///
/// # Examples
///
/// ```rust
/// set_snapshot_suffix!("suffix");
/// set_snapshot_suffix!("{}_{}", "part1", "part2");
/// ```
///
/// # Parameters
///
/// - `$expr`: One or more expressions that will be formatted and used as the snapshot suffix.
///
/// # Additional context
///
/// - This macro came from the insta documentation: <https://insta.rs/docs/patterns/>
macro_rules! set_snapshot_suffix {
    ($($expr:expr),*) => {
        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_suffix(format!($($expr,)*));
        let _guard = settings.bind_to_scope();
    }
}

pub(crate) use set_snapshot_suffix;
