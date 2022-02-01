mod version_tree;

use crate::{ParseVersionError, VersionOrder};
use std::convert::TryFrom;
use std::str::FromStr;
use thiserror::Error;
use version_tree::VersionTree;

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

    #[error("cannot join single expression")]
    CannotJoinSingleExpression,

    #[error("expression must start with '('")]
    MissingParenthesis,

    #[error("unexpected token")]
    UnexpectedOperator,

    #[error("unexpected eof")]
    UnexpectedEOF,
}

impl FromStr for VersionSpec {
    type Err = ParseVersionSpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let version_tree = VersionTree::try_from(s)?;

        Ok(VersionSpec::Any)
    }
}
