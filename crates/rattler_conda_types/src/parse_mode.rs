/// Defines how strict a parser should behave.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct ParseStrictnessWithNameMatcher {
    /// Whether to allow only `Exact` matchers for the package name or whether to also allow `Glob` or `Regex` matchers.
    pub exact_names_only: bool,
    /// Defines how strict a version parser should behave.
    pub parse_strictness: ParseStrictness,
}

impl From<ParseStrictness> for ParseStrictnessWithNameMatcher {
    fn from(value: ParseStrictness) -> Self {
        ParseStrictnessWithNameMatcher {
            exact_names_only: true,
            parse_strictness: value,
        }
    }
}

/// Defines how strict a version parser should behave.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ParseStrictness {
    /// Allows guessing the users intent.
    Lenient,

    /// Very strictly follow parsing rules.
    Strict,
}
