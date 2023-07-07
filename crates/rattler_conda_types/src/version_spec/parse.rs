use crate::version::parse::version_parser;
use crate::version_spec::constraint::Constraint;
use crate::version_spec::VersionOperator;
use crate::{ParseVersionError, ParseVersionErrorKind};
use nom::branch::alt;
use nom::bytes::complete::{tag, take_till, take_till1, take_until, take_while, take_while1};
use nom::character::complete::char;
use nom::combinator::{map, map_res, opt, value};
use nom::error::{ErrorKind, ParseError};
use nom::sequence::{delimited, terminated, tuple};
use nom::IResult;
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
    // F
    let (rest, operator_str) = take_while1(|c| "=!<>~".contains(c))(input).map_err(
        |_: nom::Err<nom::error::Error<&str>>| {
            nom::Err::Error(ParseVersionOperatorError::ExpectedOperator)
        },
    )?;

    let op = match operator_str {
        "==" => VersionOperator::Equals,
        "!=" => VersionOperator::NotEquals,
        "<=" => VersionOperator::LessEquals,
        ">=" => VersionOperator::GreaterEquals,
        "<" => VersionOperator::Less,
        ">" => VersionOperator::Greater,
        "=" => VersionOperator::StartsWith,
        "~=" => VersionOperator::Compatible,
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
    GlobVersionIncompatibleWithOperator(VersionOperator),
    #[error("regex constraints are not supported")]
    RegexConstraintsNotSupported,
    #[error("unterminated unsupported regular expression")]
    UnterminatedRegex,
    #[error("invalid operator '{0}'")]
    InvalidOperator(String),
    #[error(transparent)]
    InvalidVersion(#[from] ParseVersionErrorKind),
    /// Expected a version
    #[error("expected a version")]
    ExpectedVersion,
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
fn regex_constraint_parser(input: &str) -> IResult<&str, Constraint, ParseConstraintError> {
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
fn any_constraint_parser(input: &str) -> IResult<&str, Constraint, ParseConstraintError> {
    value(Constraint::Any, terminated(tag("*"), opt(tag(".*"))))(input)
}

/// Parses a constraint with an operator in front of it.
fn logical_constraint_parser(input: &str) -> IResult<&str, Constraint, ParseConstraintError> {
    // Parse the preceding operator
    let (input, op) = match operator_parser(input) {
        Err(
            nom::Err::Failure(ParseVersionOperatorError::InvalidOperator(op))
            | nom::Err::Error(ParseVersionOperatorError::InvalidOperator(op)),
        ) => {
            return Err(nom::Err::Failure(ParseConstraintError::InvalidOperator(
                op.to_owned(),
            )))
        }
        Err(nom::Err::Error(e)) => (input, None),
        Ok((rest, op)) => (rest, Some(op)),
        _ => unreachable!(),
    };

    // Parse the version
    let (input, version) = version_parser(input).map_err(|e| e.map(Into::into))?;

    // Parse an optional terminating glob pattern.
    let (rest, wildcard) = opt(alt((tag("*"), tag(".*"))))(input)?;

    // Convert the operator and the wildcard to something understandable
    let op = match (wildcard, op) {
        // Wildcard pattern
        (Some(_), Some(VersionOperator::StartsWith)) => VersionOperator::StartsWith,
        (Some(_), Some(VersionOperator::GreaterEquals)) => VersionOperator::GreaterEquals,
        (Some(_), Some(VersionOperator::Greater)) => VersionOperator::GreaterEquals,
        (Some(_), Some(VersionOperator::NotEquals)) => VersionOperator::NotStartsWith,
        (Some(glob), Some(op)) => {
            tracing::warn!("Using {glob} with relational operator is superfluous and deprecated and will be removed in a future version of conda.");
            op
        }
        (Some(_), None) => VersionOperator::StartsWith,

        // No wildcard, use the operator specified.
        (None, Some(op)) => op,

        // No wildcard, and no operator
        (None, None) => VersionOperator::Equals,
    };

    Ok((rest, Constraint::Comparison(op, version)))
}

/// Parses a version constraint.
fn constraint_parse(input: &str) -> IResult<&str, Constraint, ParseConstraintError> {
    alt((regex_constraint_parser, any_constraint_parser))(input)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::Version;
    use std::str::FromStr;

    #[test]
    fn test_operator_parser() {
        assert_eq!(
            operator_parser(">3.1"),
            Ok(("3.1", VersionOperator::Greater))
        );
        assert_eq!(
            operator_parser(">=3.1"),
            Ok(("3.1", VersionOperator::GreaterEquals))
        );
        assert_eq!(operator_parser("<3.1"), Ok(("3.1", VersionOperator::Less)));
        assert_eq!(
            operator_parser("<=3.1"),
            Ok(("3.1", VersionOperator::LessEquals))
        );
        assert_eq!(
            operator_parser("==3.1"),
            Ok(("3.1", VersionOperator::Equals))
        );
        assert_eq!(
            operator_parser("!=3.1"),
            Ok(("3.1", VersionOperator::NotEquals))
        );
        assert_eq!(
            operator_parser("=3.1"),
            Ok(("3.1", VersionOperator::StartsWith))
        );
        assert_eq!(
            operator_parser("~=3.1"),
            Ok(("3.1", VersionOperator::Compatible))
        );

        assert_eq!(
            operator_parser("<==>3.1"),
            Err(nom::Err::Error(ParseVersionOperatorError::InvalidOperator(
                "<==>"
            )))
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
            Err(nom::Err::Error(ParseConstraintError::UnterminatedRegex))
        );
        assert_eq!(
            regex_constraint_parser("^"),
            Err(nom::Err::Error(ParseConstraintError::UnterminatedRegex))
        );
        assert_eq!(
            regex_constraint_parser("^$"),
            Err(nom::Err::Error(
                ParseConstraintError::RegexConstraintsNotSupported
            ))
        );
        assert_eq!(
            regex_constraint_parser("^1.2.3$"),
            Err(nom::Err::Error(
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
                Constraint::Comparison(VersionOperator::Equals, Version::from_str("3.1").unwrap())
            ))
        );

        assert_eq!(
            logical_constraint_parser(">3.1"),
            Ok((
                "",
                Constraint::Comparison(VersionOperator::Greater, Version::from_str("3.1").unwrap())
            ))
        );

        assert_eq!(
            logical_constraint_parser("3.1*"),
            Ok((
                "",
                Constraint::Comparison(
                    VersionOperator::StartsWith,
                    Version::from_str("3.1").unwrap()
                )
            ))
        );

        assert_eq!(
            logical_constraint_parser("3.1.*"),
            Ok((
                "",
                Constraint::Comparison(
                    VersionOperator::StartsWith,
                    Version::from_str("3.1").unwrap()
                )
            ))
        );

        assert_eq!(
            logical_constraint_parser("~=3.1"),
            Ok((
                "",
                Constraint::Comparison(
                    VersionOperator::Compatible,
                    Version::from_str("3.1").unwrap()
                )
            ))
        );

        assert_eq!(
            logical_constraint_parser(">=3.1*"),
            Ok((
                "",
                Constraint::Comparison(
                    VersionOperator::GreaterEquals,
                    Version::from_str("3.1").unwrap()
                )
            ))
        );
    }

    #[test]
    fn parse_constraint() {
        // Regular expressions
        assert_eq!(
            constraint_parse("^1.2.3$"),
            Err(nom::Err::Failure(
                ParseConstraintError::RegexConstraintsNotSupported
            ))
        );
        assert_eq!(
            constraint_parse("^1.2.3"),
            Err(nom::Err::Failure(ParseConstraintError::UnterminatedRegex))
        );

        // Any constraints
        assert_eq!(constraint_parse("*"), Ok(("", Constraint::Any)));
        assert_eq!(constraint_parse("*.*"), Ok(("", Constraint::Any)));
    }
}
