use std::fmt::Display;

use nom::{
    branch::alt,
    bytes::complete::tag,
    character::complete::{char, multispace0},
    sequence::{delimited, preceded},
    IResult, Parser,
};
use serde::{Deserialize, Serialize};

use crate::match_spec::parse::matchspec_parser;

/// Represents a condition in a match spec, which can be a match spec itself or a logical combination
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Eq, Hash)]
pub enum MatchSpecCondition {
    /// A condition on a certain match spec (e.g. `python >=3.12`)
    MatchSpec(Box<crate::MatchSpec>),
    /// A logical AND condition combining two conditions
    And(Box<MatchSpecCondition>, Box<MatchSpecCondition>),
    /// A logical OR condition combining two conditions
    Or(Box<MatchSpecCondition>, Box<MatchSpecCondition>),
}

impl Display for MatchSpecCondition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MatchSpecCondition::MatchSpec(ms) => write!(f, "{ms}"),
            MatchSpecCondition::And(lhs, rhs) => write!(f, "({lhs} and {rhs})"),
            MatchSpecCondition::Or(lhs, rhs) => write!(f, "({lhs} or {rhs})"),
        }
    }
}

// Parse whitespace
fn ws(input: &str) -> IResult<&str, &str> {
    multispace0(input)
}

/// Check if the word-boundary delimiter `word` starts at byte position `pos` in `input`.
/// A word boundary means the character before (if any) and after (if any) must be
/// whitespace or a parenthesis.
fn check_word_delimiter(input: &str, pos: usize, word: &str) -> bool {
    let bytes = input.as_bytes();
    // Check that the word actually matches at this position
    if !input[pos..].starts_with(word) {
        return false;
    }
    let before_ok = pos == 0 || {
        let b = bytes[pos - 1];
        b.is_ascii_whitespace() || b == b'(' || b == b')'
    };
    let after_pos = pos + word.len();
    let after_ok = after_pos >= bytes.len() || {
        let b = bytes[after_pos];
        b.is_ascii_whitespace() || b == b'(' || b == b')'
    };
    before_ok && after_ok
}

// Parse a matchspec by consuming until we hit a delimiter.
// Correctly skips delimiters inside quoted strings and brackets.
fn matchspec_token(input: &str) -> IResult<&str, &str> {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut bracket_depth: u32 = 0;
    let mut in_double_quote = false;
    let mut in_single_quote = false;

    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'\\' if in_double_quote || in_single_quote => {
                // Skip the escaped character
                i += 2;
                continue;
            }
            b'"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
            }
            b'\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
            }
            b'[' if !in_double_quote && !in_single_quote => {
                bracket_depth += 1;
            }
            b']' if !in_double_quote && !in_single_quote => {
                bracket_depth = bracket_depth.saturating_sub(1);
            }
            b'(' | b')' if !in_double_quote && !in_single_quote && bracket_depth == 0 => {
                break;
            }
            _ if !in_double_quote && !in_single_quote && bracket_depth == 0 => {
                if check_word_delimiter(input, i, "and") || check_word_delimiter(input, i, "or") {
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }

    if i == 0 {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::TakeUntil,
        )));
    }

    let token = input[..i].trim();
    if token.is_empty() {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::TakeUntil,
        )));
    }

    Ok((&input[i..], token))
}

// Parse a matchspec
fn matchspec(input: &str) -> IResult<&str, MatchSpecCondition> {
    let (remaining, matchspec_str) = matchspec_token(input)?;

    match matchspec_parser(matchspec_str, crate::ParseStrictness::Strict.into()) {
        Ok(parsed_matchspec) => Ok((
            remaining,
            MatchSpecCondition::MatchSpec(Box::new(parsed_matchspec)),
        )),
        Err(_) => Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::MapRes,
        ))),
    }
}

// Parse parenthesized condition
fn parenthesized_condition(input: &str) -> IResult<&str, MatchSpecCondition> {
    delimited((char('('), ws), parse_condition, (ws, char(')'))).parse(input)
}

// Parse primary condition (matchspec or parenthesized)
fn primary_condition(input: &str) -> IResult<&str, MatchSpecCondition> {
    alt((parenthesized_condition, matchspec)).parse(input)
}

// Parse AND expressions (higher precedence)
fn and_condition(input: &str) -> IResult<&str, MatchSpecCondition> {
    let (input, first) = primary_condition(input)?;
    let (input, rest) =
        nom::multi::many0(preceded((ws, tag("and"), ws), primary_condition)).parse(input)?;

    Ok((
        input,
        rest.into_iter().fold(first, |acc, next| {
            MatchSpecCondition::And(Box::new(acc), Box::new(next))
        }),
    ))
}

// Parse OR expressions (lower precedence)
fn or_condition(input: &str) -> IResult<&str, MatchSpecCondition> {
    let (input, first) = and_condition(input)?;
    let (input, rest) =
        nom::multi::many0(preceded((ws, tag("or"), ws), and_condition)).parse(input)?;

    Ok((
        input,
        rest.into_iter().fold(first, |acc, next| {
            MatchSpecCondition::Or(Box::new(acc), Box::new(next))
        }),
    ))
}

// Parse the main condition
pub(crate) fn parse_condition(input: &str) -> IResult<&str, MatchSpecCondition> {
    or_condition(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_yaml_snapshot;

    /// Parse a condition string and assert it consumes all input.
    fn parse_full(input: &str) -> MatchSpecCondition {
        let (remaining, condition) = parse_condition(input).unwrap();
        assert_eq!(
            remaining.trim(),
            "",
            "Expected all input consumed, but got remainder: '{remaining}'"
        );
        condition
    }

    #[test]
    fn test_condition_parsing_snapshots() {
        // These are the condition expressions (extracted from the old `; if` test format).
        let test_cases = vec![
            "foobar or bizbaz",
            "python >=3.12 or foobar [version='3.12.*', url='https://foobar.com/bla.tar.bz2']",
            "foobar and (bizbaz or blabla)",
            "single_condition",
            "a and b or c",
            "(a or b) and (c or d)",
            "a and (b or (c and d))",
            "a and(b or(c and d))",
            "foobar >=1.23 *or* and(b >32.12,<=43 *and or(c and d))",
            "  foo   or   bar  ",
            "foo_bar and baz_qux",
            "(alpha and beta) or (gamma and (delta or epsilon))",
        ];

        let results: Vec<(&str, MatchSpecCondition)> = test_cases
            .into_iter()
            .map(|input| (input, parse_full(input)))
            .collect();

        assert_yaml_snapshot!(results);
    }

    #[test]
    fn test_individual_cases() {
        // Simple OR condition
        let result = parse_full("foobar or bizbaz");
        assert_yaml_snapshot!("simple_or", result);

        // Complex AND with parentheses
        let result = parse_full("foobar and (bizbaz or blabla)");
        assert_yaml_snapshot!("complex_and_with_parens", result);

        // Precedence test: AND binds tighter than OR
        let result = parse_full("a and b or c and d");
        assert_yaml_snapshot!("precedence_test", result);
    }

    #[test]
    fn test_error_cases() {
        // These should fail to parse or leave remaining input
        let error_cases = vec![
            "(unclosed_paren",
            "and missing_operand",
            "or missing_operand",
        ];

        for case in error_cases {
            let result = parse_condition(case);
            match result {
                Err(_) => {} // Expected: parse error
                Ok((remaining, _)) => {
                    assert!(
                        !remaining.trim().is_empty(),
                        "Case '{case}' should have failed or left remaining input",
                    );
                }
            }
        }
    }

    #[test]
    fn test_matchspec_token_with_quoted_and_or() {
        // "and" inside double-quoted bracket value should NOT be treated as a delimiter
        let (rem, token) = matchspec_token(r#"python[build="fast and slow"] and linux"#).unwrap();
        assert_eq!(token, r#"python[build="fast and slow"]"#);
        assert_eq!(rem, "and linux");

        // "or" inside single-quoted bracket value should NOT be treated as a delimiter
        let (rem, token) = matchspec_token("foo[version='1 or 2'] or bar").unwrap();
        assert_eq!(token, "foo[version='1 or 2']");
        assert_eq!(rem, "or bar");
    }

    #[test]
    fn test_matchspec_token_package_name_substring() {
        // "and" as substring in package name should NOT be split
        let (rem, token) = matchspec_token("pandoc >=2.0").unwrap();
        assert_eq!(token, "pandoc >=2.0");
        assert_eq!(rem, "");

        // "or" as substring in package name should NOT be split
        let (rem, token) = matchspec_token("tensorflow-core >=1.0 or linux").unwrap();
        assert_eq!(token, "tensorflow-core >=1.0");
        assert_eq!(rem, "or linux");
    }

    #[test]
    fn test_matchspec_token_brackets_without_quotes() {
        // Brackets without quotes should also be respected
        let (rem, token) = matchspec_token("foo[version>=3.6] and bar").unwrap();
        assert_eq!(token, "foo[version>=3.6]");
        assert_eq!(rem, "and bar");
    }
}
