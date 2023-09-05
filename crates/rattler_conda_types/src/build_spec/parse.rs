/// This module contains conversions to and from string representations for items defined in other parts of `build_spec` module
/// including two-way string (attempted) conversion and parsing with nom.
/// nom parsing is completely TODO
use super::{BuildNumberOperator, BuildNumberSpec};
// use crate::constraint::{parse::ParseOperatorError, OperatorConstraint};
use nom::{
    bytes::complete::take_while,
    error::{ErrorKind, ParseError},
    sequence::tuple,
    IResult,
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

impl FromStr for BuildNumberOperator {
    type Err = ParseBuildNumberSpecError;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match Self::parse(&input) {
            Ok(("", op)) => Ok(op),
            Ok((_, _)) => Err(ParseBuildNumberSpecError::ExpectedEOF),
            _ => Err(ParseBuildNumberSpecError::InvalidOperator(
                input.to_string(),
            )),
        }
    }
}

impl BuildNumberOperator {
    fn is_start_of_operator(c: char) -> bool {
        matches!(c, '>' | '<' | '=' | '!')
    }

    pub fn parse(input: &str) -> IResult<&str, Self, ParseBuildNumberSpecError> {
        use crate::constraint::{Equal, Greater, Less};
        // Take anything that looks like an operator.
        let (rest, operator_str) = take_while(Self::is_start_of_operator)(input).map_err(
            |_: nom::Err<nom::error::Error<&str>>| {
                nom::Err::Error(ParseBuildNumberSpecError::ExpectedOperator)
            },
        )?;

        let op = match operator_str {
            "==" | "" => Ok(BuildNumberOperator::Equal(Equal(true))),
            "!=" => Ok(BuildNumberOperator::Equal(Equal(false))),
            "<" => Ok(BuildNumberOperator::Less(Less(true))),
            ">=" => Ok(BuildNumberOperator::Less(Less(false))),
            ">" => Ok(BuildNumberOperator::Greater(Greater(true))),
            "<=" => Ok(BuildNumberOperator::Greater(Greater(false))),
            _ => Err(nom::Err::Failure(
                ParseBuildNumberSpecError::InvalidOperator(operator_str.to_string()),
            )),
        }?;

        Ok((rest, op))
    }
}

impl BuildNumberSpec {
    pub fn parse(input: &str) -> IResult<&str, Self, ParseBuildNumberSpecError> {
        tuple((BuildNumberOperator::parse, nom::character::complete::u64))(input)
            .map(|(rest, (op, elem))| (rest, BuildNumberSpec::new(op, elem)))
        // something needed to map the u64 error to ParseBuildNumberSpecError::ExpectedNumber
    }
}

impl FromStr for BuildNumberSpec {
    type Err = ParseBuildNumberSpecError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match Self::parse(&s) {
            Ok(("", spec)) => Ok(spec),
            Ok((_, _)) => Err(ParseBuildNumberSpecError::ExpectedEOF),
            Err(_) => unimplemented!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::constraint::{self, Equal, Greater, Less};

    use super::*;

    #[test]
    fn ordering_constraint_parse() {
        let exact = 5;
        let s: String = String::from(">=") + &exact.to_string();
        let constraint: BuildNumberSpec = BuildNumberSpec::new(
            crate::build_spec::BuildNumberOperator::Less(Less(false)),
            exact,
        );

        let repr = BuildNumberSpec::from_str(&s).unwrap();
        println!("{}", repr);
        assert_eq!(constraint, repr);
    }
}
