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

/// Options for parsing match specifications.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct ParseMatchSpecOptions {
    /// The strictness of the parser.
    strictness: ParseStrictness,

    /// Whether to allow only `Exact` matchers for the package name or whether to also allow `Glob` or `Regex` matchers.
    exact_names_only: bool,

    /// Whether to allow experimental extras syntax (e.g., `foo[extras=[bar,baz]]`).
    allow_experimental_extras: bool,

    /// Whether to allow experimental conditionals syntax (e.g., `foo; if python >=3.6`).
    allow_experimental_conditionals: bool,
}

impl ParseMatchSpecOptions {
    /// Creates a new `ParseMatchSpecOptions` with the given strictness.
    pub fn new(strictness: ParseStrictness) -> Self {
        Self {
            strictness,
            exact_names_only: true,
            allow_experimental_extras: false,
            allow_experimental_conditionals: false,
        }
    }

    /// Creates strict parsing options.
    /// Strict mode very strictly follows parsing rules.
    pub fn strict() -> Self {
        Self::new(ParseStrictness::Strict)
    }

    /// Creates lenient parsing options.
    /// Lenient mode allows guessing the user's intent.
    pub fn lenient() -> Self {
        Self::new(ParseStrictness::Lenient)
    }

    /// Returns the strictness mode.
    pub fn strictness(&self) -> ParseStrictness {
        self.strictness
    }

    /// Returns whether only exact package names are allowed.
    pub fn exact_names_only(&self) -> bool {
        self.exact_names_only
    }

    /// Returns whether experimental extras parsing is allowed.
    pub fn allow_experimental_extras(&self) -> bool {
        self.allow_experimental_extras
    }

    /// Returns whether experimental conditionals parsing is allowed.
    pub fn allow_experimental_conditionals(&self) -> bool {
        self.allow_experimental_conditionals
    }

    /// Sets whether to allow only exact package names.
    pub fn with_exact_names_only(mut self, enable: bool) -> Self {
        self.exact_names_only = enable;
        self
    }

    /// Sets whether to allow experimental extras syntax.
    pub fn with_experimental_extras(mut self, enable: bool) -> Self {
        self.allow_experimental_extras = enable;
        self
    }

    /// Sets whether to allow experimental conditionals syntax.
    pub fn with_experimental_conditionals(mut self, enable: bool) -> Self {
        self.allow_experimental_conditionals = enable;
        self
    }

    /// Sets whether to allow experimental extras syntax (mutable).
    pub fn set_experimental_extras(&mut self, enable: bool) {
        self.allow_experimental_extras = enable;
    }

    /// Sets whether to allow experimental conditionals syntax (mutable).
    pub fn set_experimental_conditionals(&mut self, enable: bool) {
        self.allow_experimental_conditionals = enable;
    }
}

impl Default for ParseMatchSpecOptions {
    fn default() -> Self {
        Self::lenient()
    }
}

impl From<ParseStrictness> for ParseMatchSpecOptions {
    fn from(strictness: ParseStrictness) -> Self {
        Self::new(strictness)
    }
}

impl From<ParseStrictnessWithNameMatcher> for ParseMatchSpecOptions {
    fn from(value: ParseStrictnessWithNameMatcher) -> Self {
        Self {
            strictness: value.parse_strictness,
            exact_names_only: value.exact_names_only,
            allow_experimental_extras: false,
            allow_experimental_conditionals: false,
        }
    }
}
