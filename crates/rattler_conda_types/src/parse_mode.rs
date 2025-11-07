/// Defines how strict a parser should behave.
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

    /// Returns whether experimental extras parsing is allowed.
    pub fn allow_experimental_extras(&self) -> bool {
        self.allow_experimental_extras
    }

    /// Returns whether experimental conditionals parsing is allowed.
    pub fn allow_experimental_conditionals(&self) -> bool {
        self.allow_experimental_conditionals
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
