mod constraint;
mod version_tree;

use crate::version_spec::constraint::{Constraint, ParseConstraintError};
use crate::version_spec::version_tree::ParseVersionTreeError;
use crate::{ParseVersionError, VersionOrder};
use serde::{Serialize, Serializer};
use std::convert::TryFrom;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use thiserror::Error;
use version_tree::VersionTree;

pub(crate) use constraint::is_start_of_version_constraint;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Serialize)]
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

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Serialize)]
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

impl Display for LogicalOperator {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            LogicalOperator::And => write!(f, ","),
            LogicalOperator::Or => write!(f, "|"),
        }
    }
}

impl Display for VersionSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        fn write(spec: &VersionSpec, f: &mut Formatter<'_>, part_of_or: bool) -> std::fmt::Result {
            match spec {
                VersionSpec::Any => write!(f, "*"),
                VersionSpec::Operator(op, version) => match op {
                    VersionOperator::StartsWith => write!(f, "{}.*", version),
                    VersionOperator::NotStartsWith => write!(f, "!={}.*", version),
                    op => write!(f, "{}{}", op, version),
                },
                VersionSpec::Group(op, group) => {
                    let requires_parenthesis = *op == LogicalOperator::And && part_of_or;
                    if requires_parenthesis {
                        write!(f, "(")?;
                    }
                    for (i, spec) in group.iter().enumerate() {
                        if i > 0 {
                            write!(f, "{}", op)?;
                        }
                        write(spec, f, *op == LogicalOperator::Or)?;
                    }
                    if requires_parenthesis {
                        write!(f, ")")?;
                    }
                    Ok(())
                }
            }
        }

        write(self, f, false)
    }
}

impl Serialize for VersionSpec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("{}", self))
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
