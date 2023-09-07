use super::constraint::{is_start_of_version_constraint, VersionConstraint};
use super::{StrictRangeOperator, VersionOperator};
use crate::constraint::operators::OrdOperator;
use crate::version::parse::{version_parser, ParseVersionError, ParseVersionErrorKind};

use nom::{
    branch::alt,
    bytes::complete::{tag, take_while, take_while1},
    character::complete::char,
    combinator::{opt, value},
    error::{ErrorKind, ParseError},
    sequence::{terminated, tuple},
    IResult,
};
use thiserror::Error;

#[derive(Debug, Clone, Error, Eq, PartialEq)]
enum ParseVersionOperatorError<'i> {
    #[error("invalid operator '{0}'")]
    InvalidOperator(&'i str),
    #[error("expected version operator")]
    ExpectedOperator,
}

/// Parses a version operator, returns an error if the operator is not recognized or not found.
fn operator_parser(input: &str) -> IResult<&str, VersionOperator, ParseVersionOperatorError> {
    // Take anything that looks like an operator.
    let (rest, operator_str) = take_while1(is_start_of_version_constraint)(input).map_err(
        |_: nom::Err<nom::error::Error<&str>>| {
            nom::Err::Error(ParseVersionOperatorError::ExpectedOperator)
        },
    )?;

    let op = match operator_str {
        "==" => VersionOperator::OrdRange(OrdOperator::Eq),
        "!=" => VersionOperator::OrdRange(OrdOperator::Ne),
        "<=" => VersionOperator::OrdRange(OrdOperator::Le),
        ">=" => VersionOperator::OrdRange(OrdOperator::Ge),
        "<" => VersionOperator::OrdRange(OrdOperator::Lt),
        ">" => VersionOperator::OrdRange(OrdOperator::Gt),
        "=" => VersionOperator::StrictRange(StrictRangeOperator::StartsWith),
        "~=" => VersionOperator::StrictRange(StrictRangeOperator::Compatible),
        _ => {
            return Err(nom::Err::Failure(
                ParseVersionOperatorError::InvalidOperator(operator_str),
            ))
        }
    };

    Ok((rest, op))
}

#[derive(Debug, Clone, Error, Eq, PartialEq)]
pub enum ParseConstraintError {
    #[error("'.' is incompatible with '{0}' operator'")]
    GlobVersionIncompatibleWithOperator(OrdOperator),
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
fn regex_constraint_parser(input: &str) -> IResult<&str, VersionConstraint, ParseConstraintError> {
    let (_rest, (_, _, terminator)) =
        tuple((char('^'), take_while(|c| c != '$'), opt(char('$'))))(input)?;
    match terminator {
        Some(_) => Err(nom::Err::Failure(
            ParseConstraintError::RegexConstraintsNotSupported,
        )),
        None => Err(nom::Err::Failure(ParseConstraintError::UnterminatedRegex)),
    }
}

/// Parses the any constraint. This matches "*" and ".*"
fn any_constraint_parser(input: &str) -> IResult<&str, VersionConstraint, ParseConstraintError> {
    value(VersionConstraint::Any, terminated(tag("*"), opt(tag(".*"))))(input)
}

/// Parses a constraint with an operator in front of it.
fn logical_constraint_parser(
    input: &str,
) -> IResult<&str, VersionConstraint, ParseConstraintError> {
    // Parse the optional preceding operator
    let (input, op) = match operator_parser(input) {
        Err(
            nom::Err::Failure(ParseVersionOperatorError::InvalidOperator(op))
            | nom::Err::Error(ParseVersionOperatorError::InvalidOperator(op)),
        ) => {
            return Err(nom::Err::Failure(ParseConstraintError::InvalidOperator(
                op.to_owned(),
            )))
        }
        Err(nom::Err::Error(_)) => (input, None),
        Ok((rest, op)) => (rest, Some(op)),
        _ => unreachable!(),
    };

    // Take everything that looks like a version and use that to parse the version. Any error means
    // no characters were detected that belong to the version.
    let (rest, version_str) = take_while1::<_, _, (&str, ErrorKind)>(|c: char| {
        c.is_alphanumeric() || "!-_.*+".contains(c)
    })(input)
    .map_err(|_| {
        nom::Err::Error(ParseConstraintError::InvalidVersion(ParseVersionError {
            kind: ParseVersionErrorKind::Empty,
            version: String::from(""),
        }))
    })?;

    // Parse the string as a version
    let (version_rest, version) = version_parser(input).map_err(|e| {
        e.map(|e| {
            ParseConstraintError::InvalidVersion(ParseVersionError {
                kind: e,
                version: String::from(""),
            })
        })
    })?;

    // Convert the operator and the wildcard to something understandable
    let op = match (version_rest, op) {
        // The version was successfully parsed
        ("", Some(op)) => op,
        ("", None) => VersionOperator::OrdRange(OrdOperator::Eq),

        // The version ends in a wildcard pattern
        ("*" | ".*", Some(VersionOperator::StrictRange(StrictRangeOperator::StartsWith))) => {
            VersionOperator::StrictRange(StrictRangeOperator::StartsWith)
        }
        ("*" | ".*", Some(VersionOperator::OrdRange(OrdOperator::Ge))) => {
            VersionOperator::OrdRange(OrdOperator::Ge)
        }
        ("*" | ".*", Some(VersionOperator::OrdRange(OrdOperator::Gt))) => {
            VersionOperator::OrdRange(OrdOperator::Ge)
        }
        ("*" | ".*", Some(VersionOperator::OrdRange(OrdOperator::Ne))) => {
            VersionOperator::StrictRange(StrictRangeOperator::NotStartsWith)
        }
        (glob @ "*" | glob @ ".*", Some(op)) => {
            tracing::warn!("Using {glob} with relational operator is superfluous and deprecated and will be removed in a future version of conda.");
            op
        }
        ("*" | ".*", None) => VersionOperator::StrictRange(StrictRangeOperator::StartsWith),

        // The version string kinda looks like a regular expression.
        (version_remainder, _) if version_str.contains('*') || version_remainder.ends_with('$') => {
            return Err(nom::Err::Error(
                ParseConstraintError::RegexConstraintsNotSupported,
            ));
        }

        // Otherwise its just a generic error.
        _ => {
            return Err(nom::Err::Error(ParseConstraintError::InvalidVersion(
                ParseVersionError {
                    version: version_str.to_owned(),
                    kind: ParseVersionErrorKind::ExpectedEof,
                },
            )))
        }
    };

    match op {
        VersionOperator::OrdRange(r) => Ok((rest, VersionConstraint::OrdComparison(r, version))),
        VersionOperator::StrictRange(s) => {
            Ok((rest, VersionConstraint::StrictComparison(s, version)))
        }
    }
}

/// Parses a version constraint.
pub(crate) fn constraint_parser(
    input: &str,
) -> IResult<&str, VersionConstraint, ParseConstraintError> {
    alt((
        regex_constraint_parser,
        any_constraint_parser,
        logical_constraint_parser,
    ))(input)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::constraint::operators::OrdOperator;
    use crate::{Version, VersionSpec};
    use std::str::FromStr;

    #[test]
    fn test_operator_parser() {
        assert_eq!(
            operator_parser(">3.1"),
            Ok(("3.1", VersionOperator::OrdRange(OrdOperator::Gt)))
        );
        assert_eq!(
            operator_parser(">=3.1"),
            Ok(("3.1", VersionOperator::OrdRange(OrdOperator::Ge)))
        );
        assert_eq!(
            operator_parser("<3.1"),
            Ok(("3.1", VersionOperator::OrdRange(OrdOperator::Lt)))
        );
        assert_eq!(
            operator_parser("<=3.1"),
            Ok(("3.1", VersionOperator::OrdRange(OrdOperator::Le)))
        );
        assert_eq!(
            operator_parser("==3.1"),
            Ok(("3.1", VersionOperator::OrdRange(OrdOperator::Eq)))
        );
        assert_eq!(
            operator_parser("!=3.1"),
            Ok(("3.1", VersionOperator::OrdRange(OrdOperator::Ne)))
        );
        assert_eq!(
            operator_parser("=3.1"),
            Ok((
                "3.1",
                VersionOperator::StrictRange(StrictRangeOperator::StartsWith)
            ))
        );
        assert_eq!(
            operator_parser("~=3.1"),
            Ok((
                "3.1",
                VersionOperator::StrictRange(StrictRangeOperator::Compatible)
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

    #[test]
    fn parse_regex_constraint() {
        assert_eq!(
            regex_constraint_parser("^.*"),
            Err(nom::Err::Failure(ParseConstraintError::UnterminatedRegex))
        );
        assert_eq!(
            regex_constraint_parser("^"),
            Err(nom::Err::Failure(ParseConstraintError::UnterminatedRegex))
        );
        assert_eq!(
            regex_constraint_parser("^$"),
            Err(nom::Err::Failure(
                ParseConstraintError::RegexConstraintsNotSupported
            ))
        );
        assert_eq!(
            regex_constraint_parser("^1.2.3$"),
            Err(nom::Err::Failure(
                ParseConstraintError::RegexConstraintsNotSupported
            ))
        );
    }

    #[test]
    fn parse_logical_constraint() {
        assert_eq!(
            logical_constraint_parser("3.1"),
            Ok((
                "",
                VersionConstraint::OrdComparison(
                    OrdOperator::Eq,
                    Version::from_str("3.1").unwrap()
                )
            ))
        );

        assert_eq!(
            logical_constraint_parser(">3.1"),
            Ok((
                "",
                VersionConstraint::OrdComparison(
                    OrdOperator::Gt,
                    Version::from_str("3.1").unwrap()
                )
            ))
        );

        assert_eq!(
            logical_constraint_parser("3.1*"),
            Ok((
                "",
                VersionConstraint::StrictComparison(
                    StrictRangeOperator::StartsWith,
                    Version::from_str("3.1").unwrap()
                )
            ))
        );

        assert_eq!(
            logical_constraint_parser("3.1.*"),
            Ok((
                "",
                VersionConstraint::StrictComparison(
                    StrictRangeOperator::StartsWith,
                    Version::from_str("3.1").unwrap()
                )
            ))
        );

        assert_eq!(
            logical_constraint_parser("~=3.1"),
            Ok((
                "",
                VersionConstraint::StrictComparison(
                    StrictRangeOperator::Compatible,
                    Version::from_str("3.1").unwrap()
                )
            ))
        );

        assert_eq!(
            logical_constraint_parser(">=3.1*"),
            Ok((
                "",
                VersionConstraint::OrdComparison(
                    OrdOperator::Ge,
                    Version::from_str("3.1").unwrap()
                )
            ))
        );
    }

    #[test]
    fn parse_constraint() {
        // Regular expressions
        assert_eq!(
            constraint_parser("^1.2.3$"),
            Err(nom::Err::Failure(
                ParseConstraintError::RegexConstraintsNotSupported
            ))
        );
        assert_eq!(
            constraint_parser("^1.2.3"),
            Err(nom::Err::Failure(ParseConstraintError::UnterminatedRegex))
        );

        // Any constraints
        assert_eq!(constraint_parser("*"), Ok(("", VersionConstraint::Any)));
        assert_eq!(constraint_parser("*.*"), Ok(("", VersionConstraint::Any)));
    }

    #[test]
    fn pixi_issue_278() {
        // TODO, move something so that from_str is in same submodule as this test
        assert!(VersionSpec::from_str("1.8.1+g6b29558").is_ok());
    }
}
