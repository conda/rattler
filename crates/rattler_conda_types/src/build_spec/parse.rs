/// This module contains conversions to and from string representations for items defined in other parts of `build_spec` module
/// including two-way string (attempted) conversion and parsing with nom.
/// nom parsing is completely TODO
use super::BuildNumberSpec;
use super::OrdOperator;
// use crate::constraint::{parse::ParseOperatorError, OperatorConstraint};
use nom::{
    bytes::complete::take_while1,
    combinator::opt,
    error::{ErrorKind, ParseError},
    sequence::tuple,
    Finish, IResult,
};
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Clone, Error, Eq, PartialEq)]
pub enum ParseBuildNumberSpecError {
    #[error("invalid operator {0}")]
    InvalidOperator(String),
    #[error("expected comparison operator")]
    ExpectedOperator,
    #[error("expected build number")]
    ExpectedNumber,
    #[error("expected EOF")]
    ExpectedEOF,
    /// Nom error
    #[error("{0:?}")]
    Nom(ErrorKind),
}

impl<'i> ParseError<&'i str> for ParseBuildNumberSpecError {
    fn from_error_kind(_: &'i str, kind: ErrorKind) -> Self {
        ParseBuildNumberSpecError::Nom(kind)
    }

    fn append(_: &'i str, _: ErrorKind, other: Self) -> Self {
        other
    }
}

impl FromStr for OrdOperator {
    type Err = ParseBuildNumberSpecError;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match Self::parse(input).finish()? {
            ("", op) => Ok(op),
            (_, _) => Err(ParseBuildNumberSpecError::ExpectedEOF),
        }
    }
}

impl OrdOperator {
    fn is_start_of_operator(c: char) -> bool {
        matches!(c, '>' | '<' | '=' | '!')
    }

    pub fn parse(input: &str) -> IResult<&str, Self, ParseBuildNumberSpecError> {
        // Take anything that looks like an operator.
        let (rest, operator_str) = take_while1(Self::is_start_of_operator)(input).map_err(
            |_: nom::Err<nom::error::Error<&str>>| {
                nom::Err::Error(ParseBuildNumberSpecError::ExpectedOperator)
            },
        )?;

        let op = match operator_str {
            "==" => Ok(OrdOperator::Eq),
            "!=" => Ok(OrdOperator::Ne),
            "<" => Ok(OrdOperator::Lt),
            ">=" => Ok(OrdOperator::Ge),
            ">" => Ok(OrdOperator::Gt),
            "<=" => Ok(OrdOperator::Le),
            _ => Err(nom::Err::Failure(
                ParseBuildNumberSpecError::InvalidOperator(operator_str.to_string()),
            )),
        }?;

        Ok((rest, op))
    }
}

impl BuildNumberSpec {
    /// parses a spec for a build number, optional operator followed by sequence of digits
    /// unrecognized operators can result in either `InvalidOperator` of `ExpectedOperator` errors
    pub fn parse(input: &str) -> IResult<&str, Self, ParseBuildNumberSpecError> {
        match tuple((opt(OrdOperator::parse), opt(nom::character::complete::u64)))(input) {
            Ok((rest, (Some(op), Some(elem)))) => Ok((rest, BuildNumberSpec::new(op, elem))),
            Ok((rest, (None, Some(elem)))) => {
                Ok((rest, BuildNumberSpec::new(OrdOperator::Eq, elem)))
            }
            Ok((_, (None, None))) => Err(nom::Err::Failure(
                ParseBuildNumberSpecError::ExpectedOperator,
            )),
            Ok((_, (Some(_), None))) => {
                Err(nom::Err::Failure(ParseBuildNumberSpecError::ExpectedNumber))
            }
            Err(nom::Err::Error(ParseBuildNumberSpecError::Nom(ErrorKind::Digit))) => Err(
                nom::Err::Failure(ParseBuildNumberSpecError::ExpectedOperator),
            ),
            Err(e) => Err(e),
        }
    }
}

impl FromStr for BuildNumberSpec {
    type Err = ParseBuildNumberSpecError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match Self::parse(s).finish() {
            Ok(("", spec)) => Ok(spec),
            Ok(_) => Err(ParseBuildNumberSpecError::ExpectedEOF),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_repr_ident() {
        for op_str in vec![">", "<", ">=", "<=", "==", "!="] {
            assert_eq!(
                OrdOperator::from_str(op_str).map(|op| format!("{}", op)),
                Ok(op_str.to_string())
            );
        }
    }

    #[test]
    fn ordering_operator_parse() {
        assert_eq!(OrdOperator::from_str(">"), Ok(OrdOperator::Gt));
        assert_eq!(OrdOperator::from_str("<"), Ok(OrdOperator::Lt));
        assert_eq!(OrdOperator::from_str(">="), Ok(OrdOperator::Ge));
        assert_eq!(OrdOperator::from_str("<="), Ok(OrdOperator::Le));
        assert_eq!(OrdOperator::from_str("=="), Ok(OrdOperator::Eq));
        assert_eq!(OrdOperator::from_str("!="), Ok(OrdOperator::Ne));
        assert_eq!(
            OrdOperator::from_str(""),
            Err(ParseBuildNumberSpecError::ExpectedOperator)
        );
    }

    #[test]
    fn ordering_constraint_success() {
        let exact = 5;

        assert_eq!(
            BuildNumberSpec::from_str(&(String::from(">=") + &exact.to_string())),
            Ok(BuildNumberSpec::new(OrdOperator::Ge, exact))
        );

        assert_eq!(
            BuildNumberSpec::from_str(&(String::from("") + &exact.to_string())),
            Ok(BuildNumberSpec::new(OrdOperator::Eq, exact))
        );
    }

    #[test]
    fn ordering_constraint_error() {
        let exact = 5;
        assert_eq!(
            BuildNumberSpec::from_str(&(String::from(">="))),
            Err(ParseBuildNumberSpecError::ExpectedNumber)
        );

        assert_eq!(
            BuildNumberSpec::from_str(&(String::from("><") + &exact.to_string())),
            Err(ParseBuildNumberSpecError::InvalidOperator("><".to_string()))
        );

        // operators not composed of unrecognizable characters are not consumed
        // and immediately fail as expect operator,
        // would it be better to take_while1 non digit and emit that?
        assert_eq!(
            BuildNumberSpec::from_str(&(String::from("~<-") + &exact.to_string())),
            Err(ParseBuildNumberSpecError::ExpectedOperator)
        );

        // parsing u64 implies operator was Eq and expect end of spec
        assert_eq!(
            (exact.to_string() + &String::from(">=")).parse::<BuildNumberSpec>(),
            Err(ParseBuildNumberSpecError::ExpectedEOF)
        );
    }
}
