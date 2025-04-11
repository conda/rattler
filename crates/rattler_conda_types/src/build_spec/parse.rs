//! this module supports the parsing features of the build number spec

use super::{BuildNumber, BuildNumberSpec, OrdOperator};

use nom::{bytes::complete::take_while1, character::complete::digit1, Finish, IResult};
use std::str::FromStr;
use thiserror::Error;

impl FromStr for BuildNumberSpec {
    type Err = ParseBuildNumberSpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match Self::parser(s).finish()? {
            ("", spec) => Ok(spec),
            (_, _) => Err(ParseBuildNumberSpecError::ExpectedEof),
        }
    }
}

impl BuildNumberSpec {
    /// Parses a build number spec, string representation is optional operator preceding whole number
    pub fn parser(input: &str) -> IResult<&str, BuildNumberSpec, ParseBuildNumberSpecError> {
        // Parse the optional preceding operator
        let (input, op) = match OrdOperator::parser(input) {
            Err(
                nom::Err::Failure(ParseOrdOperatorError::InvalidOperator(op))
                | nom::Err::Error(ParseOrdOperatorError::InvalidOperator(op)),
            ) => {
                return Err(nom::Err::Failure(
                    ParseBuildNumberSpecError::InvalidOperator(
                        ParseOrdOperatorError::InvalidOperator(op),
                    ),
                ))
            }
            Err(nom::Err::Error(_)) => (input, None),
            Ok((rest, op)) => (rest, Some(op)),
            _ => unreachable!(),
        };

        let (rest, build_num) = digit1(input)
            .map(|(rest, digits): (&str, &str)| {
                (
                    rest,
                    digits
                        .parse::<BuildNumber>()
                        .expect("nom found at least 1 digit(s)"),
                )
            })
            .map_err(|_err: nom::Err<nom::error::Error<&str>>| {
                nom::Err::Error(ParseBuildNumberSpecError::InvalidBuildNumber(
                    ParseBuildNumberError,
                ))
            })?;

        match op {
            Some(op) => Ok((rest, BuildNumberSpec::new(op, build_num))),
            None => Ok((rest, BuildNumberSpec::new(OrdOperator::Eq, build_num))),
        }
    }
}

/// Possible errors when parsing the [`OrdOperator`] which precedes the digits in a build number
/// spec.
#[derive(Debug, Clone, Error, Eq, PartialEq)]
pub enum ParseOrdOperatorError {
    /// Indicates that operator symbols were captured,
    /// but not interpretable as an `OrdOperator`
    #[error("invalid operator '{0}'")]
    InvalidOperator(String),
    /// Indicates no symbol sequence found for `OrdOperator`s,
    /// callers should expect explicit operators
    #[error("expected version operator")]
    ExpectedOperator,
    /// Indicates that there was data after an operator was read,
    /// callers should handle this if they expect input to end with the operator
    #[error("expected EOF")]
    ExpectedEof,
}

/// Simplified error when parsing the digits into a build number: u64 in a build number spec
#[derive(Debug, Clone, Eq, PartialEq, Error)]
#[error("could not parse build number")]
pub struct ParseBuildNumberError;

/// Composition of possible errors when parsing the spec for build numbers
#[allow(clippy::enum_variant_names, missing_docs)]
#[derive(Debug, Clone, Error, Eq, PartialEq)]
pub enum ParseBuildNumberSpecError {
    #[error("invalid version: {0}")]
    InvalidBuildNumber(#[source] ParseBuildNumberError),
    #[error("invalid version constraint: {0}")]
    InvalidOperator(#[source] ParseOrdOperatorError),
    #[error("expected EOF")]
    ExpectedEof,
}

impl FromStr for OrdOperator {
    type Err = ParseOrdOperatorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match Self::parser(s).finish()? {
            ("", spec) => Ok(spec),
            (_, _) => Err(ParseOrdOperatorError::ExpectedEof),
        }
    }
}

impl OrdOperator {
    /// Parses an operator representing [`PartialOrd`] compares, returns an error if the operator is
    /// not recognized or not found.
    fn parser(input: &str) -> IResult<&str, OrdOperator, ParseOrdOperatorError> {
        // Take anything that looks like an operator.
        let (rest, operator_str) = take_while1(|c| "=!<>".contains(c))(input).map_err(
            |_err: nom::Err<nom::error::Error<&str>>| {
                nom::Err::Error(ParseOrdOperatorError::ExpectedOperator)
            },
        )?;

        let op = match operator_str {
            "==" => OrdOperator::Eq,
            "!=" => OrdOperator::Ne,
            "<=" => OrdOperator::Le,
            ">=" => OrdOperator::Ge,
            "<" => OrdOperator::Lt,
            ">" => OrdOperator::Gt,
            _ => {
                return Err(nom::Err::Failure(ParseOrdOperatorError::InvalidOperator(
                    operator_str.to_string(),
                )))
            }
        };

        Ok((rest, op))
    }
}

#[cfg(test)]
mod test {
    use super::{BuildNumberSpec, OrdOperator, ParseOrdOperatorError};

    use nom::Finish;

    #[test]
    fn parse_operator_from_spec() {
        let test_params = vec![
            (">3.1", OrdOperator::Gt),
            (">=3.1", OrdOperator::Ge),
            ("<3.1", OrdOperator::Lt),
            ("<=3.1", OrdOperator::Le),
            ("==3.1", OrdOperator::Eq),
            ("!=3.1", OrdOperator::Ne),
        ];

        for (s, op) in test_params {
            assert_eq!(OrdOperator::parser(s), Ok(("3.1", op)));
        }

        assert_eq!(
            OrdOperator::parser("<==>3.1"),
            Err(nom::Err::Failure(ParseOrdOperatorError::InvalidOperator(
                "<==>".to_string()
            )))
        );
        assert_eq!(
            OrdOperator::parser("3.1"),
            Err(nom::Err::Error(ParseOrdOperatorError::ExpectedOperator))
        );
    }

    #[test]
    fn parse_spec() {
        let test_params = vec![
            (">1", OrdOperator::Gt),
            (">=1", OrdOperator::Ge),
            ("<1", OrdOperator::Lt),
            ("<=1", OrdOperator::Le),
            ("==1", OrdOperator::Eq),
            ("!=1", OrdOperator::Ne),
        ];

        for (s, op) in test_params {
            assert_eq!(
                BuildNumberSpec::parser(s),
                Ok(("", BuildNumberSpec::new(op, 1)))
            );
        }

        assert_eq!(
            BuildNumberSpec::parser(">=1.1"),
            Ok((".1", BuildNumberSpec::new(OrdOperator::Ge, 1)))
        );

        assert!(BuildNumberSpec::parser(">=build3").finish().is_err());
    }
}
