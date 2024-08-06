use itertools::Itertools;

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
