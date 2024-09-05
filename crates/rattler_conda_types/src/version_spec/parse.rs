use nom::{
    branch::alt,
    bytes::complete::{tag, take_while, take_while1},
    character::complete::char,
    combinator::opt,
    error::{ErrorKind, ParseError},
    sequence::tuple,
    IResult,
};
use thiserror::Error;

use crate::{
    version::parse::version_parser,
    version_spec::{
        constraint::Constraint, EqualityOperator, RangeOperator, StrictRangeOperator,
        VersionOperators,
    },
    ParseStrictness, ParseVersionError, ParseVersionErrorKind,
};

#[derive(Debug, Clone, Error, Eq, PartialEq)]
enum ParseVersionOperatorError<'i> {
    #[error("invalid operator '{0}'")]
    InvalidOperator(&'i str),
    #[error("expected version operator")]
    ExpectedOperator,
}

/// Parses a version operator, returns an error if the operator is not
/// recognized or not found.
fn operator_parser(input: &str) -> IResult<&str, VersionOperators, ParseVersionOperatorError<'_>> {
    // Take anything that looks like an operator.
    let (rest, operator_str) = take_while1(|c| "=!<>~".contains(c))(input).map_err(
        |_err: nom::Err<nom::error::Error<&str>>| {
            nom::Err::Error(ParseVersionOperatorError::ExpectedOperator)
        },
    )?;

    let op = match operator_str {
        "==" => VersionOperators::Exact(EqualityOperator::Equals),
        "!=" => VersionOperators::Exact(EqualityOperator::NotEquals),
        "<=" => VersionOperators::Range(RangeOperator::LessEquals),
        ">=" => VersionOperators::Range(RangeOperator::GreaterEquals),
        "<" => VersionOperators::Range(RangeOperator::Less),
        ">" => VersionOperators::Range(RangeOperator::Greater),
        "=" => VersionOperators::StrictRange(StrictRangeOperator::StartsWith),
        "~=" => VersionOperators::StrictRange(StrictRangeOperator::Compatible),
        _ => {
            return Err(nom::Err::Failure(
                ParseVersionOperatorError::InvalidOperator(operator_str),
            ));
        }
    };

    Ok((rest, op))
}

#[derive(Debug, Clone, Error, Eq, PartialEq)]
pub enum ParseConstraintError {
    #[error("'*' is incompatible with '{0}' operator'")]
    GlobVersionIncompatibleWithOperator(VersionOperators),
    #[error("regex constraints are not supported")]
    RegexConstraintsNotSupported,
    #[error("unterminated unsupported regular expression")]
    UnterminatedRegex,
    #[error("invalid operator '{0}'")]
    InvalidOperator(String),
    #[error(transparent)]
    InvalidVersion(#[from] ParseVersionError),
    /// Expected a version
    #[error("expected a version")]
    ExpectedVersion,
    /// Expected the end of the string
    #[error("encountered more characters but expected none")]
    ExpectedEof,
    /// Nom error
    #[error("{0:?}")]
    Nom(ErrorKind),

    #[error("invalid glob pattern")]
    InvalidGlob,
}

impl<'i> ParseError<&'i str> for ParseConstraintError {
    fn from_error_kind(_: &'i str, kind: ErrorKind) -> Self {
        ParseConstraintError::Nom(kind)
    }

    fn append(_: &'i str, _: ErrorKind, other: Self) -> Self {
        other
    }
}

/// Parses a regex constraint. Returns an error if no terminating `$` is found.
fn regex_constraint_parser(
    _strictness: ParseStrictness,
) -> impl FnMut(&str) -> IResult<&str, Constraint, ParseConstraintError> {
    move |input: &str| {
        let (_rest, (preceder, _, terminator)) =
            tuple((opt(char('^')), take_while(|c| c != '$'), opt(char('$'))))(input)?;
        match (preceder, terminator) {
            (None, None) => Err(nom::Err::Error(ParseConstraintError::UnterminatedRegex)),
            (_, None) | (None, _) => {
                Err(nom::Err::Failure(ParseConstraintError::UnterminatedRegex))
            }
            _ => Err(nom::Err::Failure(
                ParseConstraintError::RegexConstraintsNotSupported,
            )),
        }
    }
}

/// Parses an any constraint. This matches "*" and ".*".
fn any_constraint_parser(
    strictness: ParseStrictness,
) -> impl FnMut(&str) -> IResult<&str, Constraint, ParseConstraintError> {
    move |input: &str| {
        let (remaining, (_, trailing)) = tuple((tag("*"), opt(tag(".*"))))(input)?;

        // `*.*` is not allowed in strict mode
        if trailing.is_some() && strictness == ParseStrictness::Strict {
            return Err(nom::Err::Failure(ParseConstraintError::InvalidGlob));
        }

        Ok((remaining, Constraint::Any))
    }
}

/// Parses a constraint with an operator in front of it.
fn logical_constraint_parser(
    strictness: ParseStrictness,
) -> impl FnMut(&str) -> IResult<&str, Constraint, ParseConstraintError> {
    use ParseStrictness::{Lenient, Strict};

    move |input: &str| {
        // Parse the optional preceding operator
        let (input, op) = match operator_parser(input) {
            Err(
                nom::Err::Failure(ParseVersionOperatorError::InvalidOperator(op))
                | nom::Err::Error(ParseVersionOperatorError::InvalidOperator(op)),
            ) => {
                return Err(nom::Err::Failure(ParseConstraintError::InvalidOperator(
                    op.to_owned(),
                )));
            }
            Err(nom::Err::Error(_)) => (input, None),
            Ok((rest, op)) => (rest, Some(op)),
            _ => unreachable!(),
        };

        // Take everything that looks like a version and use that to parse the version.
        // Any error means no characters were detected that belong to the
        // version.
        let (rest, version_str) = take_while1::<_, _, (&str, ErrorKind)>(|c: char| {
            c.is_alphanumeric() || "!-_.*+".contains(c)
        })(input)
        .map_err(|_err| {
            nom::Err::Error(ParseConstraintError::InvalidVersion(ParseVersionError {
                kind: ParseVersionErrorKind::Empty,
                version: String::from(""),
            }))
        })?;

        // Handle the case where no version was specified. These cases don't make any
        // sense (e.g. ``>=*``) but they do exist in the wild. This code here
        // tries to map it to something that at least makes some sort of sense.
        // But this is not the case for everything, for instance what
        // what is meant with `!=*` or `<*`?
        // See: https://github.com/AnacondaRecipes/repodata-hotfixes/issues/220
        if version_str == "*" {
            let op = op.expect(
                "if no operator was specified for the star then this is not a logical constraint",
            );

            if strictness == Strict {
                return Err(nom::Err::Failure(
                    ParseConstraintError::GlobVersionIncompatibleWithOperator(op),
                ));
            }

            return match op {
                VersionOperators::Range(
                    RangeOperator::GreaterEquals | RangeOperator::LessEquals,
                )
                | VersionOperators::StrictRange(
                    StrictRangeOperator::Compatible | StrictRangeOperator::StartsWith,
                )
                | VersionOperators::Exact(EqualityOperator::Equals) => Ok((rest, Constraint::Any)),
                op => {
                    return Err(nom::Err::Failure(
                        ParseConstraintError::GlobVersionIncompatibleWithOperator(op),
                    ));
                }
            };
        }

        // Parse the string as a version
        let (version_rest, version) = version_parser(version_str).map_err(|e| {
            e.map(|e| {
                ParseConstraintError::InvalidVersion(ParseVersionError {
                    kind: e,
                    version: String::from(""),
                })
            })
        })?;

        // Convert the operator and the wildcard to something understandable
        let op = match (version_rest, op, strictness) {
            // The version was successfully parsed
            ("", Some(op), _) => op,
            ("", None, _) => VersionOperators::Exact(EqualityOperator::Equals),

            // The version ends in a wildcard pattern
            (
                "*" | ".*",
                Some(VersionOperators::Range(
                    RangeOperator::GreaterEquals | RangeOperator::Greater,
                )),
                Lenient,
            ) => VersionOperators::Range(RangeOperator::GreaterEquals),
            (
                "*" | ".*",
                Some(VersionOperators::Exact(EqualityOperator::NotEquals)),
                Lenient | Strict,
            ) => {
                // !=1.2.3.* is the only way to express a version should not start with 1.2.3 so
                // even in strict mode we allow this.
                VersionOperators::StrictRange(StrictRangeOperator::NotStartsWith)
            }
            ("*" | ".*", Some(op), Lenient) => {
                // In lenient mode we simply ignore the glob.
                op
            }
            ("*" | ".*", Some(op), Strict) => {
                return Err(nom::Err::Failure(
                    ParseConstraintError::GlobVersionIncompatibleWithOperator(op),
                ));
            }
            ("*" | ".*", None, _) => VersionOperators::StrictRange(StrictRangeOperator::StartsWith),

            // Support for edge case version spec that looks like `2023.*.*`.
            (version_remainder, None, Lenient)
                if looks_like_infinite_starts_with(version_remainder) =>
            {
                VersionOperators::StrictRange(StrictRangeOperator::StartsWith)
            }

            // The version string kinda looks like a regular expression.
            (version_remainder, _, _)
                if version_str.contains('*') || version_remainder.ends_with('$') =>
            {
                return Err(nom::Err::Failure(
                    ParseConstraintError::RegexConstraintsNotSupported,
                ));
            }

            // Otherwise its just a generic error.
            _ => {
                return Err(nom::Err::Failure(ParseConstraintError::InvalidVersion(
                    ParseVersionError {
                        version: version_str.to_owned(),
                        kind: ParseVersionErrorKind::ExpectedEof,
                    },
                )));
            }
        };

        match op {
            VersionOperators::Range(r) => Ok((rest, Constraint::Comparison(r, version))),
            VersionOperators::Exact(e) => Ok((rest, Constraint::Exact(e, version))),
            VersionOperators::StrictRange(s) => {
                Ok((rest, Constraint::StrictComparison(s, version)))
            }
        }
    }
}

/// Returns true if the input looks like a version constraint with repeated
/// starts with pattern. E.g: `.*.*`
///
/// This is an edge case found in the anaconda main repodata as `mkl 2023.*.*`.
pub fn looks_like_infinite_starts_with(input: &str) -> bool {
    let mut input = input.strip_suffix('.').unwrap_or(input);
    while !input.is_empty() {
        match input.strip_suffix(".*") {
            Some("") => {
                // If we were able to continuously strip the `.*` pattern,
                // then we found a match.
                return true;
            }
            Some(rest) => {
                input = rest;
            }
            None => return false,
        }
    }

    false
}

/// Parses a version constraint.
pub fn constraint_parser(
    strictness: ParseStrictness,
) -> impl FnMut(&str) -> IResult<&str, Constraint, ParseConstraintError> {
    move |input| {
        alt((
            regex_constraint_parser(strictness),
            any_constraint_parser(strictness),
            logical_constraint_parser(strictness),
        ))(input)
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use assert_matches::assert_matches;
    use rstest::rstest;

    use super::*;
    use crate::{ParseStrictness::*, Version, VersionSpec};

    #[test]
    fn test_operator_parser() {
        assert_eq!(
            operator_parser(">3.1"),
            Ok(("3.1", VersionOperators::Range(RangeOperator::Greater)))
        );
        assert_eq!(
            operator_parser(">=3.1"),
            Ok(("3.1", VersionOperators::Range(RangeOperator::GreaterEquals)))
        );
        assert_eq!(
            operator_parser("<3.1"),
            Ok(("3.1", VersionOperators::Range(RangeOperator::Less)))
        );
        assert_eq!(
            operator_parser("<=3.1"),
            Ok(("3.1", VersionOperators::Range(RangeOperator::LessEquals)))
        );
        assert_eq!(
            operator_parser("==3.1"),
            Ok(("3.1", VersionOperators::Exact(EqualityOperator::Equals)))
        );
        assert_eq!(
            operator_parser("!=3.1"),
            Ok(("3.1", VersionOperators::Exact(EqualityOperator::NotEquals)))
        );
        assert_eq!(
            operator_parser("=3.1"),
            Ok((
                "3.1",
                VersionOperators::StrictRange(StrictRangeOperator::StartsWith)
            ))
        );
        assert_eq!(
            operator_parser("~=3.1"),
            Ok((
                "3.1",
                VersionOperators::StrictRange(StrictRangeOperator::Compatible)
            ))
        );

        assert_eq!(
            operator_parser("<==>3.1"),
            Err(nom::Err::Failure(
                ParseVersionOperatorError::InvalidOperator("<==>")
            ))
        );
        assert_eq!(
            operator_parser("3.1"),
            Err(nom::Err::Error(ParseVersionOperatorError::ExpectedOperator))
        );
    }

    #[rstest]
    fn parse_regex(#[values(Lenient, Strict)] strictness: ParseStrictness) {
        assert_eq!(
            regex_constraint_parser(strictness)("^.*"),
            Err(nom::Err::Failure(ParseConstraintError::UnterminatedRegex))
        );
        assert_eq!(
            regex_constraint_parser(strictness)("^"),
            Err(nom::Err::Failure(ParseConstraintError::UnterminatedRegex))
        );
        assert_eq!(
            regex_constraint_parser(strictness)("^$"),
            Err(nom::Err::Failure(
                ParseConstraintError::RegexConstraintsNotSupported
            ))
        );
        assert_eq!(
            regex_constraint_parser(strictness)("^1.2.3$"),
            Err(nom::Err::Failure(
                ParseConstraintError::RegexConstraintsNotSupported
            ))
        );
    }

    #[rstest]
    fn parse_logical_constraint(#[values(Lenient, Strict)] strictness: ParseStrictness) {
        assert_eq!(
            logical_constraint_parser(strictness)("3.1"),
            Ok((
                "",
                Constraint::Exact(EqualityOperator::Equals, Version::from_str("3.1").unwrap())
            ))
        );

        assert_eq!(
            logical_constraint_parser(strictness)(">3.1"),
            Ok((
                "",
                Constraint::Comparison(RangeOperator::Greater, Version::from_str("3.1").unwrap())
            ))
        );

        assert_eq!(
            logical_constraint_parser(strictness)("3.1*"),
            Ok((
                "",
                Constraint::StrictComparison(
                    StrictRangeOperator::StartsWith,
                    Version::from_str("3.1").unwrap(),
                )
            ))
        );

        assert_eq!(
            logical_constraint_parser(strictness)("3.1.*"),
            Ok((
                "",
                Constraint::StrictComparison(
                    StrictRangeOperator::StartsWith,
                    Version::from_str("3.1").unwrap(),
                )
            ))
        );

        assert_eq!(
            logical_constraint_parser(strictness)("~=3.1"),
            Ok((
                "",
                Constraint::StrictComparison(
                    StrictRangeOperator::Compatible,
                    Version::from_str("3.1").unwrap(),
                )
            ))
        );
    }

    #[test]
    fn parse_logical_constraint_lenient() {
        assert_eq!(
            logical_constraint_parser(Lenient)(">=3.1*"),
            Ok((
                "",
                Constraint::Comparison(
                    RangeOperator::GreaterEquals,
                    Version::from_str("3.1").unwrap(),
                )
            ))
        );
        assert_matches!(
            logical_constraint_parser(Strict)(">=3.1*"),
            Err(nom::Err::Failure(
                ParseConstraintError::GlobVersionIncompatibleWithOperator(_)
            ))
        );
    }

    #[rstest]
    fn parse_regex_constraint(#[values(Lenient, Strict)] strictness: ParseStrictness) {
        // Regular expressions
        assert_eq!(
            constraint_parser(strictness)("^1.2.3$"),
            Err(nom::Err::Failure(
                ParseConstraintError::RegexConstraintsNotSupported
            ))
        );
        assert_eq!(
            constraint_parser(strictness)("^1.2.3"),
            Err(nom::Err::Failure(ParseConstraintError::UnterminatedRegex))
        );

        // Any constraints
        assert_eq!(
            constraint_parser(strictness)("*"),
            Ok(("", Constraint::Any))
        );
    }

    #[rstest]
    fn parse_any_constraint(#[values(Lenient, Strict)] strictness: ParseStrictness) {
        assert_eq!(
            constraint_parser(strictness)("*"),
            Ok(("", Constraint::Any))
        );
    }

    #[test]
    fn parse_any_constraint_lenient() {
        assert_eq!(constraint_parser(Lenient)("*.*"), Ok(("", Constraint::Any)));
        assert_matches!(
            constraint_parser(Strict)("*.*"),
            Err(nom::Err::Failure(ParseConstraintError::InvalidGlob))
        );
    }

    #[test]
    fn pixi_issue_278() {
        assert!(VersionSpec::from_str("1.8.1+g6b29558", Strict).is_ok());
    }

    #[test]
    fn test_looks_like_infinite_starts_with() {
        assert!(looks_like_infinite_starts_with(".*"));
        assert!(looks_like_infinite_starts_with(".*.*"));
        assert!(looks_like_infinite_starts_with(".*."));
        assert!(!looks_like_infinite_starts_with("."));
        assert!(!looks_like_infinite_starts_with(".0.*"));
        assert!(!looks_like_infinite_starts_with(""));
        assert!(!looks_like_infinite_starts_with(""));
        assert!(!looks_like_infinite_starts_with(".* .*"));
    }
}
