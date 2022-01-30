use crate::{ParseVersionError, VersionOrder};
use nom::branch::alt;
use nom::bytes::complete::tag;
use nom::combinator::{map, map_res, opt, value};
use nom::error::{context, ErrorKind};
use nom::multi::many1;
use nom::sequence::{delimited, pair};
use nom::{IResult, InputTakeAtPosition};
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum VersionOperator {
    Equals,
    NotEquals,
    Greater,
    GreaterEquals,
    Less,
    LessEquals,
    StartsWith,
    NotStartsWith,
    Compatible,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum LogicalOperator {
    And,
    Or,
}

#[derive(Debug, Clone)]
pub enum VersionSpec {
    Any,
    Operator(VersionOperator, VersionOrder),
    Group(LogicalOperator, Vec<VersionSpec>),
}

#[derive(Debug, Error)]
pub enum ParseVersionSpecError {
    #[error("operator followed by space")]
    OperatorFollowedBySpace,

    #[error("invalid version")]
    InvalidVersion(#[source] ParseVersionError),
}

impl FromStr for VersionSpec {
    type Err = ParseVersionSpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(VersionSpec::Any)
    }
}

fn parse_version_operator(input: &str) -> IResult<&str, VersionOperator> {
    alt((
        value(VersionOperator::NotStartsWith, tag("!=startswith")),
        value(VersionOperator::Compatible, tag("~=")),
        value(VersionOperator::Equals, tag("==")),
        value(VersionOperator::NotEquals, tag("!=")),
        value(VersionOperator::GreaterEquals, tag(">=")),
        value(VersionOperator::LessEquals, tag("<=")),
        value(VersionOperator::Greater, tag(">")),
        value(VersionOperator::Less, tag("<")),
        value(VersionOperator::StartsWith, tag("=")),
    ))(input)
}

fn parse_version_order(input: &str) -> IResult<&str, VersionOrder> {
    pub fn parse_version_string(input: &str) -> IResult<&str, &str> {
        input.split_at_position1_complete(
            |c| !crate::version::is_valid_char(c),
            ErrorKind::RegexpMatch,
        )
    }

    map_res(
        context("Version", parse_version_string),
        VersionOrder::from_str,
    )(input)
}

fn parse_simple_version_spec(input: &str) -> IResult<&str, (VersionOperator, VersionOrder)> {
    pair(
        map(
            opt(parse_version_operator),
            |op: Option<VersionOperator>| op.unwrap_or(VersionOperator::StartsWith),
        ),
        parse_version_order,
    )(input)
}

#[cfg(test)]
mod tests {
    use super::parse_version_operator;
    use crate::version_spec::{parse_simple_version_spec, VersionOperator};

    #[test]
    fn version_operator() {
        assert_eq!(
            parse_version_operator(">"),
            Ok(("", VersionOperator::Greater))
        );
        assert_eq!(parse_version_operator("<"), Ok(("", VersionOperator::Less)));
        assert_eq!(
            parse_version_operator("<="),
            Ok(("", VersionOperator::LessEquals))
        );
        assert_eq!(
            parse_version_operator(">="),
            Ok(("", VersionOperator::GreaterEquals))
        );
        assert_eq!(
            parse_version_operator("=="),
            Ok(("", VersionOperator::Equals))
        );
        assert_eq!(
            parse_version_operator("!="),
            Ok(("", VersionOperator::NotEquals))
        );
        assert_eq!(
            parse_version_operator("!=startswith"),
            Ok(("", VersionOperator::NotStartsWith))
        );
        assert_eq!(
            parse_version_operator("~="),
            Ok(("", VersionOperator::Compatible))
        );
        assert_eq!(
            parse_version_operator("="),
            Ok(("", VersionOperator::StartsWith))
        );
    }

    #[test]
    fn simple_version_spec() {
        let (_, (op, version)) = parse_simple_version_spec("1.0").unwrap();
        assert_eq!(op, VersionOperator::StartsWith);
        assert_eq!(version.to_string(), "1.0");

        let (_, (op, version)) = parse_simple_version_spec("!=2.1").unwrap();
        assert_eq!(op, VersionOperator::NotEquals);
        assert_eq!(version.to_string(), "2.1");
    }
}
