mod constraint;
mod version_tree;

use crate::version_spec::constraint::{Constraint, ParseConstraintError};
use crate::version_spec::version_tree::ParseVersionTreeError;
use crate::{ParseVersionError, VersionOrder};
use std::convert::TryFrom;
use std::fmt::{Display, Formatter};
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

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum VersionSpec {
    Any,
    Operator(VersionOperator, VersionOrder),
    Group(LogicalOperator, Vec<VersionSpec>),
}

#[derive(Debug, Clone, Eq, PartialEq, Error)]
pub enum ParseVersionSpecError {
    #[error("invalid version")]
    InvalidVersion(#[source] ParseVersionError),

    #[error("invalid version tree")]
    InvalidVersionTree(#[source] ParseVersionTreeError),

    #[error("invalid version constraint")]
    InvalidConstraint(#[source] ParseConstraintError),
}

impl FromStr for VersionSpec {
    type Err = ParseVersionSpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let version_tree =
            VersionTree::try_from(s).map_err(ParseVersionSpecError::InvalidVersionTree)?;

        fn parse_tree(tree: VersionTree) -> Result<VersionSpec, ParseVersionSpecError> {
            match tree {
                VersionTree::Term(str) => {
                    let constraint = Constraint::from_str(str)
                        .map_err(ParseVersionSpecError::InvalidConstraint)?;
                    Ok(match constraint {
                        Constraint::Any => VersionSpec::Any,
                        Constraint::Comparison(op, ver) => VersionSpec::Operator(op, ver),
                    })
                }
                VersionTree::Group(op, groups) => Ok(VersionSpec::Group(
                    op,
                    groups
                        .into_iter()
                        .map(parse_tree)
                        .collect::<Result<_, ParseVersionSpecError>>()?,
                )),
            }
        }

        parse_tree(version_tree)
    }
}

impl Display for VersionOperator {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionOperator::Equals => write!(f, "=="),
            VersionOperator::NotEquals => write!(f, "!="),
            VersionOperator::Greater => write!(f, ">"),
            VersionOperator::GreaterEquals => write!(f, ">="),
            VersionOperator::Less => write!(f, "<"),
            VersionOperator::LessEquals => write!(f, "<="),
            VersionOperator::StartsWith => write!(f, "="),
            VersionOperator::NotStartsWith => write!(f, "!=startswith"),
            VersionOperator::Compatible => write!(f, "~="),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::version_spec::{LogicalOperator, VersionOperator};
    use crate::{VersionOrder, VersionSpec};
    use std::str::FromStr;

    #[test]
    fn test_simple() {
        assert_eq!(
            VersionSpec::from_str("1.2.3"),
            Ok(VersionSpec::Operator(
                VersionOperator::Equals,
                VersionOrder::from_str("1.2.3").unwrap()
            ))
        );
        assert_eq!(
            VersionSpec::from_str(">=1.2.3"),
            Ok(VersionSpec::Operator(
                VersionOperator::GreaterEquals,
                VersionOrder::from_str("1.2.3").unwrap()
            ))
        );
    }
    #[test]
    fn test_group() {
        assert_eq!(
            VersionSpec::from_str(">=1.2.3,<2.0.0"),
            Ok(VersionSpec::Group(
                LogicalOperator::And,
                vec![
                    VersionSpec::Operator(
                        VersionOperator::GreaterEquals,
                        VersionOrder::from_str("1.2.3").unwrap()
                    ),
                    VersionSpec::Operator(
                        VersionOperator::Less,
                        VersionOrder::from_str("2.0.0").unwrap()
                    ),
                ]
            ))
        );
        assert_eq!(
            VersionSpec::from_str(">=1.2.3|<1.0.0"),
            Ok(VersionSpec::Group(
                LogicalOperator::Or,
                vec![
                    VersionSpec::Operator(
                        VersionOperator::GreaterEquals,
                        VersionOrder::from_str("1.2.3").unwrap()
                    ),
                    VersionSpec::Operator(
                        VersionOperator::Less,
                        VersionOrder::from_str("1.0.0").unwrap()
                    ),
                ]
            ))
        );
        assert_eq!(
            VersionSpec::from_str("((>=1.2.3)|<1.0.0)"),
            Ok(VersionSpec::Group(
                LogicalOperator::Or,
                vec![
                    VersionSpec::Operator(
                        VersionOperator::GreaterEquals,
                        VersionOrder::from_str("1.2.3").unwrap()
                    ),
                    VersionSpec::Operator(
                        VersionOperator::Less,
                        VersionOrder::from_str("1.0.0").unwrap()
                    ),
                ]
            ))
        );
    }
}
