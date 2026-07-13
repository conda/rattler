use itertools::Itertools;

/// An error returned when a string cannot be safely used as a single filesystem
/// path component.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "the value {value:?} cannot be used as a path component because it could allow path traversal"
)]
pub struct InvalidPathComponentError {
    /// The offending value.
    pub value: String,
}

/// Rejects a string that could escape its parent directory when interpolated
/// into a filesystem path.
///
/// Rejects parent-directory references and path/drive/ADS separators. None of
/// these are valid in a conda `name` or `build`, so well-formed packages are
/// unaffected (GHSA-h672-p7h7-97v9).
pub fn ensure_safe_path_component(component: &str) -> Result<(), InvalidPathComponentError> {
    let is_unsafe = component == "."
        || component == ".."
        || component
            .chars()
            .any(|c| matches!(c, '/' | '\\' | ':' | '\0') || c.is_control());

    if is_unsafe {
        Err(InvalidPathComponentError {
            value: component.to_string(),
        })
    } else {
        Ok(())
    }
}

/// Returns true if the specified string is considered to be an absolute path
pub(crate) fn is_absolute_path(path: &str) -> bool {
    if path.contains("://") {
        return false;
    }

    // Check if the path starts with a common absolute path prefix
    if path.starts_with('/') || path.starts_with("\\\\") {
        return true;
    }

    // A drive letter followed by a colon and a (backward or forward) slash
    matches!(path.chars().take(3).collect_tuple(),
        Some((letter, ':', '/' | '\\')) if letter.is_alphabetic())
}

/// Returns true if the specified string is considered to be a path
pub(crate) fn is_path(path: &str) -> bool {
    if path.contains("://") {
        return false;
    }

    // Check if the path starts with a common path prefix
    if path.starts_with("./")
        || path.starts_with("..")
        || path.starts_with("~/")
        || path.starts_with('/')
        || path.starts_with("\\\\")
        || path.starts_with("//")
    {
        return true;
    }

    // A drive letter followed by a colon and a (backward or forward) slash
    matches!(path.chars().take(3).collect_tuple(),
        Some((letter, ':', '/' | '\\')) if letter.is_alphabetic())
}

mod tests {
    #[test]
    fn test_ensure_safe_path_component() {
        use super::ensure_safe_path_component;

        // Legitimate conda cache directory / file names.
        assert!(ensure_safe_path_component("demo-1.0-py39h6fdeb60_14").is_ok());
        assert!(ensure_safe_path_component("openssl-3.0.0-h0123456_0.json").is_ok());
        assert!(ensure_safe_path_component("pkg-1!2.3+local-0").is_ok());

        // Traversal and separators must be rejected.
        assert!(ensure_safe_path_component(r"x\..\..\..\project\.git\hooks").is_err());
        assert!(ensure_safe_path_component("a/b").is_err());
        assert!(ensure_safe_path_component("a\\b").is_err());
        assert!(ensure_safe_path_component("..").is_err());
        assert!(ensure_safe_path_component(".").is_err());
        // An empty component (e.g. an empty build string) cannot traverse.
        assert!(ensure_safe_path_component("").is_ok());
        // Windows drive / alternate-data-stream separator.
        assert!(ensure_safe_path_component("C:evil").is_err());
        // Control characters / NUL.
        assert!(ensure_safe_path_component("a\0b").is_err());
        assert!(ensure_safe_path_component("a\nb").is_err());
    }

    #[test]
    fn test_is_absolute_path() {
        use super::is_absolute_path;
        assert!(is_absolute_path("/foo"));
        assert!(is_absolute_path("/C:/foo"));
        assert!(is_absolute_path("C:/foo"));
        assert!(is_absolute_path("\\\\foo"));
        assert!(is_absolute_path("\\\\server\\foo"));

        assert!(!is_absolute_path("conda-forge/label/rust_dev"));
        assert!(!is_absolute_path("~/foo"));
        assert!(!is_absolute_path("./foo"));
        assert!(!is_absolute_path("../foo"));
        assert!(!is_absolute_path("foo"));
        assert!(!is_absolute_path("~\\foo"));
    }

    #[test]
    fn test_is_path() {
        use super::is_path;
        assert!(is_path("/foo"));
        assert!(is_path("/C:/foo"));
        assert!(is_path("C:/foo"));
        assert!(is_path("\\\\foo"));
        assert!(is_path("\\\\server\\foo"));

        assert!(is_path("./conda-forge/label/rust_dev"));
        assert!(is_path("~/foo"));
        assert!(is_path("./foo"));
        assert!(is_path("../foo"));

        assert!(!is_path("~\\foo"));
    }
}
