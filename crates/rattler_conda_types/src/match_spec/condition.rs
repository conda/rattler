use std::fmt::Display;

use nom::{
    IResult, Parser,
    branch::alt,
    bytes::complete::tag,
    character::complete::{char, multispace0},
    sequence::{delimited, preceded},
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
            MatchSpecCondition::MatchSpec(ms) => ms.fmt_in_condition(f),
            MatchSpecCondition::And(lhs, rhs) => write!(f, "({lhs} and {rhs})"),
            MatchSpecCondition::Or(lhs, rhs) => write!(f, "({lhs} or {rhs})"),
        }
    }
}

impl MatchSpecCondition {
    /// Renders a canonical condition with only the parentheses required to
    /// preserve precedence and the exact shape of this left-associative AST.
    pub(crate) fn to_canonical_string(&self) -> Result<String, crate::CanonicalMatchSpecError> {
        self.to_canonical_string_with_parent(0, false)
    }

    fn to_canonical_string_with_parent(
        &self,
        parent_precedence: u8,
        is_right_child: bool,
    ) -> Result<String, crate::CanonicalMatchSpecError> {
        let precedence = match self {
            Self::MatchSpec(_) => 3,
            Self::And(_, _) => 2,
            Self::Or(_, _) => 1,
        };
        let mut value = match self {
            Self::MatchSpec(match_spec) => {
                let leaf = match_spec.to_canonical_condition_string()?;
                let options = crate::ParseMatchSpecOptions::strict()
                    .with_repodata_revision(crate::RepodataRevision::V3)
                    .with_exact_names_only(false);
                if !matches!(
                    parse_condition_with_options(&leaf, options),
                    Ok((rest, Self::MatchSpec(_))) if rest.trim().is_empty()
                ) {
                    return Err(crate::CanonicalMatchSpecError::UnrepresentableConditionLeaf(leaf));
                }
                leaf
            }
            Self::And(lhs, rhs) => format!(
                "{} and {}",
                lhs.to_canonical_string_with_parent(precedence, false)?,
                rhs.to_canonical_string_with_parent(precedence, true)?,
            ),
            Self::Or(lhs, rhs) => format!(
                "{} or {}",
                lhs.to_canonical_string_with_parent(precedence, false)?,
                rhs.to_canonical_string_with_parent(precedence, true)?,
            ),
        };

        if precedence < parent_precedence
            || (is_right_child && precedence == parent_precedence && precedence < 3)
        {
            value = format!("({value})");
        }
        Ok(value)
    }
}

// Parse whitespace
fn ws(input: &str) -> IResult<&str, &str> {
    multispace0(input)
}

/// Checks whether `word` starts at a condition-token boundary.
fn check_word_delimiter(input: &str, position: usize, word: &str) -> bool {
    let Some(remainder) = input.get(position..) else {
        return false;
    };
    if !remainder.starts_with(word) {
        return false;
    }

    input[..position]
        .chars()
        .next_back()
        .is_none_or(|character| character.is_whitespace() || matches!(character, '(' | ')'))
        && remainder[word.len()..]
            .chars()
            .next()
            .is_none_or(|character| character.is_whitespace() || matches!(character, '(' | ')'))
}

/// Consumes one `MatchSpec` leaf without splitting logical words inside quoted
/// bracket fields. Iterating over characters keeps UTF-8 boundaries valid.
fn matchspec_token(input: &str) -> IResult<&str, &str> {
    let mut characters = input.char_indices().peekable();
    let mut end = input.len();
    let mut bracket_depth = 0_u32;
    let mut quote = None;

    while let Some((position, character)) = characters.next() {
        if let Some(quote_character) = quote {
            match character {
                '\\' if characters.next().is_none() => {
                    return Err(nom::Err::Error(nom::error::Error::new(
                        input,
                        nom::error::ErrorKind::Escaped,
                    )));
                }
                '\\' => {}
                character if character == quote_character => quote = None,
                _ => {}
            }
            continue;
        }

        match character {
            '\'' | '"' if bracket_depth > 0 => quote = Some(character),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '(' | ')' if bracket_depth == 0 => {
                end = position;
                break;
            }
            _ if bracket_depth == 0
                && (check_word_delimiter(input, position, "and")
                    || check_word_delimiter(input, position, "or")) =>
            {
                end = position;
                break;
            }
            _ => {}
        }
    }

    let token = input[..end].trim();
    if token.is_empty() {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::TakeUntil,
        )));
    }

    Ok((&input[end..], token))
}

fn matchspec(
    input: &str,
    options: crate::ParseMatchSpecOptions,
) -> IResult<&str, MatchSpecCondition> {
    let (remaining, matchspec_str) = matchspec_token(input)?;
    let mut leaf_options = options;
    leaf_options.set_conditionals(false);

    match matchspec_parser(matchspec_str, leaf_options) {
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

fn parenthesized_condition(
    input: &str,
    options: crate::ParseMatchSpecOptions,
) -> IResult<&str, MatchSpecCondition> {
    delimited(
        (char('('), ws),
        |input| parse_condition_with_options(input, options),
        (ws, char(')')),
    )
    .parse(input)
}

fn primary_condition(
    input: &str,
    options: crate::ParseMatchSpecOptions,
) -> IResult<&str, MatchSpecCondition> {
    alt((
        |input| parenthesized_condition(input, options),
        |input| matchspec(input, options),
    ))
    .parse(input)
}

fn and_condition(
    input: &str,
    options: crate::ParseMatchSpecOptions,
) -> IResult<&str, MatchSpecCondition> {
    let (input, first) = primary_condition(input, options)?;
    let (input, rest) = nom::multi::many0(preceded((ws, tag("and"), ws), |input| {
        primary_condition(input, options)
    }))
    .parse(input)?;

    Ok((
        input,
        rest.into_iter().fold(first, |acc, next| {
            MatchSpecCondition::And(Box::new(acc), Box::new(next))
        }),
    ))
}

fn or_condition(
    input: &str,
    options: crate::ParseMatchSpecOptions,
) -> IResult<&str, MatchSpecCondition> {
    let (input, first) = and_condition(input, options)?;
    let (input, rest) = nom::multi::many0(preceded((ws, tag("or"), ws), |input| {
        and_condition(input, options)
    }))
    .parse(input)?;

    Ok((
        input,
        rest.into_iter().fold(first, |acc, next| {
            MatchSpecCondition::Or(Box::new(acc), Box::new(next))
        }),
    ))
}

pub(crate) fn parse_condition_with_options(
    input: &str,
    options: crate::ParseMatchSpecOptions,
) -> IResult<&str, MatchSpecCondition> {
    or_condition(input, options)
}

#[cfg(test)]
pub(crate) fn parse_condition(input: &str) -> IResult<&str, MatchSpecCondition> {
    parse_condition_with_options(input, crate::ParseMatchSpecOptions::strict())
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
