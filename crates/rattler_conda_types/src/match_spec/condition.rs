use std::fmt::Display;

use nom::{
    branch::alt,
    bytes::complete::{tag, take_while1},
    character::complete::{char, multispace0, multispace1},
    sequence::{delimited, preceded},
    IResult, Parser,
};
use serde::Serialize;

use crate::match_spec::parse::matchspec_parser;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Statement {
    pub prefix: String,
    pub condition: Option<MatchSpecCondition>,
}

/// Represents a condition in a match spec, which can be a match spec itself or a logical combination
#[derive(Debug, Clone, PartialEq, Serialize, Eq, Hash)]
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
            MatchSpecCondition::MatchSpec(ms) => write!(f, "{}", ms),
            MatchSpecCondition::And(lhs, rhs) => write!(f, "({} and {})", lhs, rhs),
            MatchSpecCondition::Or(lhs, rhs) => write!(f, "({} or {})", lhs, rhs),
        }
    }
}

// Parse identifier (alphanumeric + underscore)
fn identifier(input: &str) -> IResult<&str, &str> {
    take_while1(|c: char| c.is_alphanumeric() || c == '_')(input)
}

// Parse whitespace
fn ws(input: &str) -> IResult<&str, &str> {
    multispace0(input)
}

// Parse a matchspec by consuming until we hit a delimiter
fn matchspec_token(input: &str) -> IResult<&str, &str> {
    // Try to find the next delimiter
    let delimiters = ["and", "or", ")", "("];

    // Find the earliest delimiter
    let mut end_pos = input.len();
    for delimiter in &delimiters {
        if let Some(pos) = input.find(delimiter) {
            // Make sure it's a word boundary for "and"/"or"
            if *delimiter == "and" || *delimiter == "or" {
                // Check if it's preceded and followed by whitespace or start/end of string
                let is_word_boundary = {
                    let before_ok = pos == 0 || input.chars().nth(pos - 1).unwrap().is_whitespace();
                    let after_ok = pos + delimiter.len() >= input.len()
                        || input
                            .chars()
                            .nth(pos + delimiter.len())
                            .unwrap()
                            .is_whitespace();
                    before_ok && after_ok
                };
                if is_word_boundary {
                    end_pos = end_pos.min(pos);
                }
            } else {
                end_pos = end_pos.min(pos);
            }
        }
    }

    if end_pos == 0 {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::TakeUntil,
        )));
    }

    let (matchspec_str, remaining) = input.split_at(end_pos);
    let matchspec_str = matchspec_str.trim();

    if matchspec_str.is_empty() {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::TakeUntil,
        )));
    }

    Ok((remaining, matchspec_str))
}

// Parse a matchspec
fn matchspec(input: &str) -> IResult<&str, MatchSpecCondition> {
    let (remaining, matchspec_str) = matchspec_token(input)?;

    match matchspec_parser(matchspec_str, crate::ParseStrictness::Strict) {
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
        nom::multi::many0(preceded((ws, tag("and"), multispace1), primary_condition))
            .parse(input)?;

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
        nom::multi::many0(preceded((ws, tag("or"), multispace1), and_condition)).parse(input)?;

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
    use nom::combinator::opt;

    fn parse_and_extract(input: &str) -> Statement {
        let result = parse_statement(input).unwrap();
        assert_eq!(result.0.trim(), ""); // Ensure no remaining input
        result.1
    }

    // Parse the entire statement
    pub fn parse_statement(input: &str) -> IResult<&str, Statement> {
        let (input, prefix) = identifier(input)?;
        let (input, _) = char(';')(input)?;
        let (input, _) = ws(input)?;
        let (input, condition) =
            opt(preceded((tag("if"), multispace1), parse_condition)).parse(input)?;

        Ok((
            input,
            Statement {
                prefix: prefix.to_string(),
                condition,
            },
        ))
    }

    #[test]
    fn test_condition_parsing_snapshots() {
        let test_cases = vec![
            "bla; if foobar or bizbaz",
            "bla; if python >=3.12 or foobar [version='3.12.*', url='https://foobar.com/bla.tar.bz2']",
            "xyz; if foobar and (bizbaz or blabla)",
            "test;",
            "simple; if single_condition",
            "complex; if a and b or c",
            "nested; if (a or b) and (c or d)",
            "deep; if a and (b or (c and d))",
            "whitespace;   if   foo   or   bar  ",
            "underscores; if foo_bar and baz_qux",
            "mixed; if (alpha and beta) or (gamma and (delta or epsilon))",
        ];

        let results: Vec<(&str, Statement)> = test_cases
            .into_iter()
            .map(|input| (input, parse_and_extract(input)))
            .collect();

        assert_yaml_snapshot!(results);
    }

    #[test]
    fn test_individual_cases() {
        // Simple OR condition
        let result = parse_and_extract("bla; if foobar or bizbaz");
        assert_yaml_snapshot!("simple_or", result);

        // Complex AND with parentheses
        let result = parse_and_extract("xyz; if foobar and (bizbaz or blabla)");
        assert_yaml_snapshot!("complex_and_with_parens", result);

        // No condition
        let result = parse_and_extract("test;");
        assert_yaml_snapshot!("no_condition", result);

        // Precedence test
        let result = parse_and_extract("prec; if a and b or c and d");
        assert_yaml_snapshot!("precedence_test", result);
    }

    #[test]
    fn test_error_cases() {
        // These should fail to parse completely
        let error_cases = vec![
            "no_semicolon if foo",
            "; if missing_prefix",
            "bad; if (unclosed_paren",
            "bad; if closed_paren)",
            "bad; if and missing_operand",
            "bad; if or missing_operand",
        ];

        for case in error_cases {
            let result = parse_statement(case);
            // These should either fail or not consume all input
            if let Ok((remaining, _)) = result {
                assert!(
                    !remaining.is_empty(),
                    "Case '{}' should have failed or left remaining input",
                    case
                );
            }
        }
    }
}
