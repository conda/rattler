use crate::version::parse::version_parser;
use crate::version_spec::constraint::Constraint;
use crate::version_spec::VersionOperator;
use crate::{ParseVersionError, ParseVersionErrorKind};
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
        ("", None) => VersionOperator::Equals,

        // The version ends in a wildcard pattern
        ("*" | ".*", Some(VersionOperator::StartsWith)) => VersionOperator::StartsWith,
        ("*" | ".*", Some(VersionOperator::GreaterEquals)) => VersionOperator::GreaterEquals,
        ("*" | ".*", Some(VersionOperator::Greater)) => VersionOperator::GreaterEquals,
        ("*" | ".*", Some(VersionOperator::NotEquals)) => VersionOperator::NotStartsWith,
        (glob @ "*" | glob @ ".*", Some(op)) => {
            tracing::warn!("Using {glob} with relational operator is superfluous and deprecated and will be removed in a future version of conda.");
            op
        }
        ("*" | ".*", None) => VersionOperator::StartsWith,

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

    Ok((rest, Constraint::Comparison(op, version)))
}

/// Parses a version constraint.
pub(crate) fn constraint_parser(input: &str) -> IResult<&str, Constraint, ParseConstraintError> {
    alt((
        regex_constraint_parser,
        any_constraint_parser,
        logical_constraint_parser,
    ))(input)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{Version, VersionSpec};
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
        assert_eq!(constraint_parser("*"), Ok(("", Constraint::Any)));
        assert_eq!(constraint_parser("*.*"), Ok(("", Constraint::Any)));
    }

    #[test]
    fn pixi_issue_278() {
        assert!(VersionSpec::from_str("1.8.1+g6b29558").is_ok());
    }
}
