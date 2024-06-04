use itertools::Itertools;

/// Returns true if the specified string is considered to be a path
pub(crate) fn is_path(path: &str) -> bool {
    if path.contains("://") {
        return false;
    }

    // Check if the path starts with a common path prefix
    if path.starts_with("./")
        || path.starts_with("..")
        || path.starts_with('~')
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
