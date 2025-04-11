use once_cell::sync::Lazy;
use regex::Regex;
use unicode_normalization::UnicodeNormalization;

/// Converts a given text to a slug.
///
/// This function performs the following steps:
/// 1. Normalizes the text using Unicode normalization and removes non-ASCII characters.
/// 2. Removes special characters, converts the text to lowercase, and trims whitespace.
/// 3. Replaces whitespace and hyphens with a single hyphen.
pub fn slugify(text: &str) -> String {
    static RE_SPECIAL: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^\w\s-]").expect("Invalid regex"));
    static RE_SPACES: Lazy<Regex> = Lazy::new(|| Regex::new(r"[_\s-]+").expect("Invalid regex"));

    // Normalize the text and remove non-ASCII characters
    let normalized = text.nfkd().filter(char::is_ascii).collect::<String>();

    // Remove special characters, convert to lowercase, and trim
    let without_special = RE_SPECIAL.replace_all(&normalized, "").to_string();
    let trimmed = without_special.trim().to_lowercase();

    // Replace whitespace and hyphens with a single hyphen
    RE_SPACES.replace_all(&trimmed, "-").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_slugify() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("Hello, World!"), "hello-world");
    }

    #[test]
    fn test_special_characters() {
        assert_eq!(
            slugify("Hello, World! How are you?"),
            "hello-world-how-are-you"
        );
    }

    #[test]
    fn test_multiple_spaces() {
        assert_eq!(
            slugify("This   has   many   spaces"),
            "this-has-many-spaces"
        );
    }

    #[test]
    fn test_non_ascii_characters() {
        assert_eq!(slugify("Héllö Wörld"), "hello-world");
    }

    #[test]
    fn test_leading_trailing_spaces() {
        assert_eq!(slugify("  Trim me  "), "trim-me");
    }
}
