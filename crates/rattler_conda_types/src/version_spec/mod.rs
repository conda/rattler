//! This module contains code to work with "versionspec". It represents the version part of
//! [`crate::MatchSpec`], e.g.: `>=3.4,<4.0`.

mod constraint;
pub(crate) mod version_tree;

use crate::version_spec::constraint::{Constraint, ParseConstraintError};
use crate::version_spec::version_tree::ParseVersionTreeError;
use crate::{ParseVersionError, Version};
use serde::{Serialize, Serializer};
use std::convert::TryFrom;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use thiserror::Error;
use version_tree::VersionTree;

pub(crate) use constraint::is_start_of_version_constraint;

/// An operator to compare two versions.
#[allow(missing_docs)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize)]
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
    NotCompatible,
}

impl VersionOperator {
    /// Returns the complement of the current operator.
    pub fn complement(self) -> Self {
        match self {
            VersionOperator::Equals => VersionOperator::NotEquals,
            VersionOperator::NotEquals => VersionOperator::Equals,
            VersionOperator::Greater => VersionOperator::LessEquals,
            VersionOperator::GreaterEquals => VersionOperator::Less,
            VersionOperator::Less => VersionOperator::GreaterEquals,
            VersionOperator::LessEquals => VersionOperator::Greater,
            VersionOperator::StartsWith => VersionOperator::NotStartsWith,
            VersionOperator::NotStartsWith => VersionOperator::StartsWith,
            VersionOperator::Compatible => VersionOperator::NotCompatible,
            VersionOperator::NotCompatible => VersionOperator::Compatible,
        }
    }
}

/// Logical operator used two compare groups of version comparisions. E.g. `>=3.4,<4.0` or
/// `>=3.4|<4.0`,
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize)]
pub enum LogicalOperator {
    /// All comparators must evaluate to true for the group to evaluate to true.
    And,

    /// Any comparators must evaluate to true for the group to evaluate to true.
    Or,
}

impl LogicalOperator {
    /// Returns the complement of the operator.
    pub fn complement(self) -> Self {
        match self {
            LogicalOperator::And => LogicalOperator::Or,
            LogicalOperator::Or => LogicalOperator::And,
        }
    }
}

/// A version specification.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum VersionSpec {
    /// No version specified
    None,
    /// Any version
    Any,
    /// A specific version
    Operator(VersionOperator, Version),
    /// A group of version specifications
    Group(LogicalOperator, Vec<VersionSpec>),
}

#[allow(clippy::enum_variant_names, missing_docs)]
#[derive(Debug, Clone, Eq, PartialEq, Error)]
pub enum ParseVersionSpecError {
    #[error("invalid version: {0}")]
    InvalidVersion(#[source] ParseVersionError),

    #[error("invalid version tree: {0}")]
    InvalidVersionTree(#[source] ParseVersionTreeError),

    #[error("invalid version constraint: {0}")]
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
            VersionOperator::NotCompatible => write!(f, "!~="),
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
                VersionSpec::None => write!(f, "!"),
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

impl VersionSpec {
    /// Returns whether the version matches the specification.
    pub fn matches(&self, version: &Version) -> bool {
        match self {
            VersionSpec::None => false,
            VersionSpec::Any => true,
            VersionSpec::Operator(VersionOperator::Equals, limit) => limit == version,
            VersionSpec::Operator(VersionOperator::NotEquals, limit) => limit != version,
            VersionSpec::Operator(VersionOperator::Greater, limit) => version > limit,
            VersionSpec::Operator(VersionOperator::GreaterEquals, limit) => version >= limit,
            VersionSpec::Operator(VersionOperator::Less, limit) => version < limit,
            VersionSpec::Operator(VersionOperator::LessEquals, limit) => version <= limit,
            VersionSpec::Operator(VersionOperator::StartsWith, limit) => version.starts_with(limit),
            VersionSpec::Operator(VersionOperator::NotStartsWith, limit) => {
                !version.starts_with(limit)
            }
            VersionSpec::Operator(VersionOperator::Compatible, limit) => version >= limit && version.starts_with(&limit.remove_last_element()),
            VersionSpec::Operator(VersionOperator::NotCompatible, limit) => version < limit || !version.starts_with(&limit.remove_last_element()),
            VersionSpec::Group(LogicalOperator::And, group) => {
                group.iter().all(|spec| spec.matches(version))
            }
            VersionSpec::Group(LogicalOperator::Or, group) => {
                group.iter().any(|spec| spec.matches(version))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::version_spec::{LogicalOperator, VersionOperator};
    use crate::{Version, VersionSpec};
    use std::str::FromStr;

    #[test]
    fn test_simple() {
        assert_eq!(
            VersionSpec::from_str("1.2.3"),
            Ok(VersionSpec::Operator(
                VersionOperator::Equals,
                Version::from_str("1.2.3").unwrap()
            ))
        );
        assert_eq!(
            VersionSpec::from_str(">=1.2.3"),
            Ok(VersionSpec::Operator(
                VersionOperator::GreaterEquals,
                Version::from_str("1.2.3").unwrap()
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
                        Version::from_str("1.2.3").unwrap()
                    ),
                    VersionSpec::Operator(
                        VersionOperator::Less,
                        Version::from_str("2.0.0").unwrap()
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
                        Version::from_str("1.2.3").unwrap()
                    ),
                    VersionSpec::Operator(
                        VersionOperator::Less,
                        Version::from_str("1.0.0").unwrap()
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
                        Version::from_str("1.2.3").unwrap()
                    ),
                    VersionSpec::Operator(
                        VersionOperator::Less,
                        Version::from_str("1.0.0").unwrap()
                    ),
                ]
            ))
        );
    }

    #[test]
    fn test_matches() {
        let v1 = Version::from_str("1.2.0").unwrap();
        let vs1 = VersionSpec::from_str(">=1.2.3,<2.0.0").unwrap();
        assert!(!vs1.matches(&v1));

        let vs2 = VersionSpec::from_str("1.2").unwrap();
        assert!(vs2.matches(&v1));

        let v2 = Version::from_str("1.2.3").unwrap();
        assert!(vs1.matches(&v2));
        assert!(!vs2.matches(&v2));

        let v3 = Version::from_str("1!1.2.3").unwrap();
        println!("{:?}", v3);

        assert!(!vs1.matches(&v3));
        assert!(!vs2.matches(&v3));

        let vs3 = VersionSpec::from_str(">=1!1.2,<1!2").unwrap();
        assert!(vs3.matches(&v3));
    }

    #[test]
    fn test_compatible_matches() {
        let v1 = Version::from_str("2.2.0").unwrap();
        let v1x = Version::from_str("2.2.1").unwrap();
        let v1y = Version::from_str("2.3.0").unwrap();
        let v1z = Version::from_str("2.20.32213").unwrap();
        let v2 = Version::from_str("3.2.0").unwrap();

        let vs1 = VersionSpec::from_str("~=2.2").unwrap();
        assert!(vs1.matches(&v1));
        assert!(vs1.matches(&v1x));
        assert!(vs1.matches(&v1y));
        assert!(vs1.matches(&v1z));
        assert!(!vs1.matches(&v2));

        let vs2 = VersionSpec::from_str("~=2.2,<4").unwrap();
        assert!(vs2.matches(&v1));
        assert!(!vs2.matches(&v2));
    }

    #[test]
    fn issue_204() {
        assert!(VersionSpec::from_str(">=3.8<3.9").is_err());
    }
}
