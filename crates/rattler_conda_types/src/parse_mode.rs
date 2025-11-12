/// Defines how strict a parser should behave.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct ParseStrictnessWithNameMatcher {
    /// Whether to allow only `Exact` matchers for the package name or whether to also allow `Glob` or `Regex` matchers.
    pub exact_names_only: bool,
    /// Defines how strict a version parser should behave.
    pub parse_strictness: ParseStrictness,
}

impl Into<ParseStrictnessWithNameMatcher> for ParseStrictness {
    fn into(self) -> ParseStrictnessWithNameMatcher {
        match self {
            ParseStrictness::Lenient => ParseStrictnessWithNameMatcher {
                exact_names_only: true,
                parse_strictness: ParseStrictness::Lenient,
            },
            ParseStrictness::Strict => ParseStrictnessWithNameMatcher {
                exact_names_only: true,
                parse_strictness: ParseStrictness::Strict,
            },
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
