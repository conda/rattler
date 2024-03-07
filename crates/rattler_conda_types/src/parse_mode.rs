/// Defines how strict a parser should behave.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ParseStrictness {
    /// Allows guessing the users intent.
    Lenient,

    /// Very strictly follow parsing rules.
    Strict,
}
