use regex::Regex;
use unicode_normalization::UnicodeNormalization;

pub fn slugify(text: &str) -> String {
    // Normalize the text and remove non-ASCII characters
    let normalized = text.nfkd().filter(char::is_ascii).collect::<String>();

    // Remove special characters, convert to lowercase, and trim
    let re_special = Regex::new(r"[^\w\s-]").unwrap();
    let without_special = re_special.replace_all(&normalized, "").to_string();
    let trimmed = without_special.trim().to_lowercase();

    // Replace whitespace and hyphens with a single hyphen
    let re_spaces = Regex::new(r"[_\s-]+").unwrap();
    re_spaces.replace_all(&trimmed, "-").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_slugify() {
        assert_eq!(slugify("Hello World"), "hello-world");
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
