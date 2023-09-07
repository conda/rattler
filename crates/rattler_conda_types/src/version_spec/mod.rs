//! This module contains code to work with "versionspec". It represents the version part of
//! [`crate::MatchSpec`], e.g.: `>=3.4,<4.0`.

mod constraint;
pub(crate) mod parse;
pub(crate) mod version_tree;

use crate::constraint::operators::OrdOperator;
use crate::version_spec::version_tree::ParseVersionTreeError;
use crate::{ParseVersionError, Version};
pub(crate) use constraint::VersionConstraint;
use serde::{Deserialize, Serialize, Serializer};
use std::convert::TryFrom;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use thiserror::Error;
use version_tree::VersionTree;

use crate::version::StrictVersion;
pub(crate) use parse::ParseConstraintError;

#[allow(missing_docs)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum StrictRangeOperator {
    StartsWith,
    NotStartsWith,
    Compatible,
    NotCompatible,
}

impl StrictRangeOperator {
    /// Returns the complement of the current operator.
    pub fn complement(self) -> Self {
        match self {
            StrictRangeOperator::StartsWith => StrictRangeOperator::NotStartsWith,
            StrictRangeOperator::NotStartsWith => StrictRangeOperator::StartsWith,
            StrictRangeOperator::Compatible => StrictRangeOperator::NotCompatible,
            StrictRangeOperator::NotCompatible => StrictRangeOperator::Compatible,
        }
    }
}

/// Range and equality operators combined
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize)]
pub enum VersionOperator {
    /// Specifies a range of versions using the strict operator
    StrictRange(StrictRangeOperator),
    /// Uses methods from impl Ord to specify a version
    OrdRange(OrdOperator),
}

impl VersionOperator {
    /// returns boolean if char is one of those that could parse to a single or multiple character VersionOperator
    pub fn is_start_of_operator(c: char) -> bool {
        matches!(c, '>' | '<' | '=' | '!' | '~')
    }
}

/// Logical operator used two compare groups of version comparisions. E.g. `>=3.4,<4.0` or
/// `>=3.4|<4.0`,
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum LogicalOperator {
    /// All comparators must evaluate to true for the group to evaluate to true.
    And,

    /// Any comparators must evaluate to true for the group to evaluate to true.
    Or,
}

impl LogicalOperator {
    // what kind of complement is this?
    // shouldn't this return (in a haskell-ish syntax) `logicalOp . map not`?
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
#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Deserialize)]
pub enum VersionSpec {
    /// No version specified
    None,
    /// Any version
    Any,
    /// A version range
    OrdRange(OrdOperator, Version),
    /// A version range using the strict operator
    StrictRange(StrictRangeOperator, StrictVersion),
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
                    let constraint = VersionConstraint::from_str(str)
                        .map_err(ParseVersionSpecError::InvalidConstraint)?;
                    Ok(match constraint {
                        VersionConstraint::Any => VersionSpec::Any,
                        VersionConstraint::OrdComparison(op, ver) => VersionSpec::OrdRange(op, ver),
                        VersionConstraint::StrictComparison(op, ver) => {
                            VersionSpec::StrictRange(op, StrictVersion(ver))
                        }
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

impl Display for StrictRangeOperator {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            StrictRangeOperator::StartsWith => write!(f, "="),
            StrictRangeOperator::NotStartsWith => write!(f, "!=startswith"),
            StrictRangeOperator::Compatible => write!(f, "~="),
            StrictRangeOperator::NotCompatible => write!(f, "!~="),
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
                VersionSpec::StrictRange(op, version) => match op {
                    StrictRangeOperator::StartsWith => write!(f, "{}.*", version),
                    StrictRangeOperator::NotStartsWith => write!(f, "!={}.*", version),
                    op => write!(f, "{}{}", op, version),
                },
                VersionSpec::OrdRange(op, version) => {
                    write!(f, "{}{}", op, version)
                }
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
            VersionSpec::OrdRange(OrdOperator::Eq, limit) => limit == version,
            VersionSpec::OrdRange(OrdOperator::Ne, limit) => limit != version,
            VersionSpec::OrdRange(OrdOperator::Gt, limit) => version > limit,
            VersionSpec::OrdRange(OrdOperator::Ge, limit) => version >= limit,
            VersionSpec::OrdRange(OrdOperator::Lt, limit) => version < limit,
            VersionSpec::OrdRange(OrdOperator::Le, limit) => version <= limit,
            VersionSpec::StrictRange(StrictRangeOperator::StartsWith, limit) => {
                version.starts_with(&limit.0)
            }
            VersionSpec::StrictRange(StrictRangeOperator::NotStartsWith, limit) => {
                !version.starts_with(&limit.0)
            }
            VersionSpec::StrictRange(StrictRangeOperator::Compatible, limit) => {
                version.compatible_with(&limit.0)
            }
            VersionSpec::StrictRange(StrictRangeOperator::NotCompatible, limit) => {
                !version.compatible_with(&limit.0)
            }
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
    use crate::version_spec::LogicalOperator;
    use crate::version_spec::OrdOperator;
    use crate::{Version, VersionSpec};
    use std::str::FromStr;

    #[test]
    fn test_simple() {
        assert_eq!(
            VersionSpec::from_str("1.2.3"),
            Ok(VersionSpec::OrdRange(
                OrdOperator::Eq,
                Version::from_str("1.2.3").unwrap()
            ))
        );
        assert_eq!(
            VersionSpec::from_str(">=1.2.3"),
            Ok(VersionSpec::OrdRange(
                OrdOperator::Ge,
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
                    VersionSpec::OrdRange(OrdOperator::Ge, Version::from_str("1.2.3").unwrap()),
                    VersionSpec::OrdRange(OrdOperator::Lt, Version::from_str("2.0.0").unwrap()),
                ]
            ))
        );
        assert_eq!(
            VersionSpec::from_str(">=1.2.3|<1.0.0"),
            Ok(VersionSpec::Group(
                LogicalOperator::Or,
                vec![
                    VersionSpec::OrdRange(OrdOperator::Ge, Version::from_str("1.2.3").unwrap()),
                    VersionSpec::OrdRange(OrdOperator::Lt, Version::from_str("1.0.0").unwrap()),
                ]
            ))
        );
        assert_eq!(
            VersionSpec::from_str("((>=1.2.3)|<1.0.0)"),
            Ok(VersionSpec::Group(
                LogicalOperator::Or,
                vec![
                    VersionSpec::OrdRange(OrdOperator::Ge, Version::from_str("1.2.3").unwrap()),
                    VersionSpec::OrdRange(OrdOperator::Lt, Version::from_str("1.0.0").unwrap()),
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
    fn issue_204() {
        assert!(VersionSpec::from_str(">=3.8<3.9").is_err());
    }

    #[test]
    fn issue_225() {
        let spec = VersionSpec::from_str("~=2.4").unwrap();
        assert!(!spec.matches(&Version::from_str("3.1").unwrap()));
        assert!(spec.matches(&Version::from_str("2.4").unwrap()));
        assert!(spec.matches(&Version::from_str("2.5").unwrap()));
        assert!(!spec.matches(&Version::from_str("2.1").unwrap()));
    }

    #[test]
    fn issue_235() {
        assert_eq!(
            VersionSpec::from_str(">2.10*").unwrap(),
            VersionSpec::from_str(">=2.10").unwrap()
        );
    }
}
