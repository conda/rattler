/// Controls whether parsers accept legacy or ambiguous conda syntax.
///
/// Use [`ParseStrictness::Strict`] when accepting only the documented conda
/// syntax. Use [`ParseStrictness::Lenient`] when parsing existing user input or
/// metadata that may use historical syntax.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ParseStrictness {
    /// Accept compatible historical and ambiguous syntax where it has a
    /// well-defined interpretation.
    Lenient,

    /// Reject syntax that is not part of the documented grammar.
    Strict,
}
