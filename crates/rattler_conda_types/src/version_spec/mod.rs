//! This module contains code to work with "versionspec". It represents the
//! version part of [`crate::MatchSpec`], e.g.: `>=3.4,<4.0`.

mod constraint;
pub(crate) mod parse;
pub(crate) mod version_tree;

use std::{
    convert::TryFrom,
    fmt::{Display, Formatter},
    str::FromStr,
};

pub(crate) use constraint::is_start_of_version_constraint;
use constraint::Constraint;
use parse::ParseConstraintError;
use serde::{Deserialize, Serialize, Serializer};
use thiserror::Error;
use version_tree::VersionTree;

use crate::{
    version::StrictVersion, version_spec::version_tree::ParseVersionTreeError, ParseStrictness,
    ParseVersionError, Version,
};

/// An operator to compare two versions.
#[allow(missing_docs)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum RangeOperator {
    Greater,
    GreaterEquals,
    Less,
    LessEquals,
}

impl RangeOperator {
    /// Returns the complement of the current operator.
    pub fn complement(self) -> Self {
        match self {
            RangeOperator::Greater => RangeOperator::LessEquals,
            RangeOperator::GreaterEquals => RangeOperator::Less,
            RangeOperator::Less => RangeOperator::GreaterEquals,
            RangeOperator::LessEquals => RangeOperator::Greater,
        }
    }
}

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

/// An operator set a version equal to another
#[allow(missing_docs)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum EqualityOperator {
    Equals,
    NotEquals,
}

impl EqualityOperator {
    /// Returns the complement of the current operator.
    pub fn complement(self) -> Self {
        match self {
            EqualityOperator::Equals => EqualityOperator::NotEquals,
            EqualityOperator::NotEquals => EqualityOperator::Equals,
        }
    }
}

/// Range and equality operators combined
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize)]
pub enum VersionOperators {
    /// Specifies a range of versions
    Range(RangeOperator),
    /// Specifies a range of versions using the strict operator
    StrictRange(StrictRangeOperator),
    /// Specifies an exact version
    Exact(EqualityOperator),
}

/// Logical operator used two compare groups of version comparisons. E.g.
/// `>=3.4,<4.0` or `>=3.4|<4.0`,
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Deserialize)]
pub enum VersionSpec {
    /// No version specified
    None,
    /// Any version
    Any,
    /// A version range
    Range(RangeOperator, Version),
    /// A version range using the strict operator
    StrictRange(StrictRangeOperator, StrictVersion),
    /// A exact version
    Exact(EqualityOperator, Version),
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

impl From<Constraint> for VersionSpec {
    fn from(constraint: Constraint) -> Self {
        match constraint {
            Constraint::Any => VersionSpec::Any,
            Constraint::Comparison(op, ver) => VersionSpec::Range(op, ver),
            Constraint::StrictComparison(op, ver) => {
                VersionSpec::StrictRange(op, StrictVersion(ver))
            }
            Constraint::Exact(e, ver) => VersionSpec::Exact(e, ver),
        }
    }
}

impl FromStr for VersionSpec {
    type Err = ParseVersionSpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        VersionSpec::from_str(s, ParseStrictness::Lenient)
    }
}

impl VersionSpec {
    /// Parse a [`VersionSpec`] from a string.
    pub fn from_str(
        source: &str,
        strictness: ParseStrictness,
    ) -> Result<Self, ParseVersionSpecError> {
        fn parse_tree(
            tree: VersionTree<'_>,
            strictness: ParseStrictness,
        ) -> Result<VersionSpec, ParseVersionSpecError> {
            match tree {
                VersionTree::Term(str) => Ok(Constraint::from_str(str, strictness)
                    .map_err(ParseVersionSpecError::InvalidConstraint)?
                    .into()),
                VersionTree::Group(op, groups) => Ok(VersionSpec::Group(
                    op,
                    groups
                        .into_iter()
                        .map(|group| parse_tree(group, strictness))
                        .collect::<Result<_, ParseVersionSpecError>>()?,
                )),
            }
        }

        let version_tree =
            VersionTree::try_from(source).map_err(ParseVersionSpecError::InvalidVersionTree)?;

        parse_tree(version_tree, strictness)
    }
}

impl Display for VersionOperators {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionOperators::Range(r) => write!(f, "{r}"),
            VersionOperators::StrictRange(r) => write!(f, "{r}"),
            VersionOperators::Exact(r) => write!(f, "{r}"),
        }
    }
}

impl Display for RangeOperator {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RangeOperator::Greater => write!(f, ">"),
            RangeOperator::GreaterEquals => write!(f, ">="),
            RangeOperator::Less => write!(f, "<"),
            RangeOperator::LessEquals => write!(f, "<="),
        }
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

impl Display for EqualityOperator {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Equals => write!(f, "=="),
            Self::NotEquals => write!(f, "!="),
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
        fn write(
            spec: &VersionSpec,
            f: &mut Formatter<'_>,
            parent_op: Option<LogicalOperator>,
        ) -> std::fmt::Result {
            match spec {
                VersionSpec::Any => write!(f, "*"),
                VersionSpec::StrictRange(op, version) => match op {
                    StrictRangeOperator::StartsWith => write!(f, "{version}.*"),
                    StrictRangeOperator::NotStartsWith => write!(f, "!={version}.*"),
                    op => write!(f, "{op}{version}"),
                },
                VersionSpec::Range(op, version) => {
                    write!(f, "{op}{version}")
                }
                VersionSpec::Exact(op, version) => {
                    write!(f, "{op}{version}")
                }
                VersionSpec::Group(op, group) => {
                    let requires_parenthesis = matches!(
                        (op, parent_op),
                        (LogicalOperator::Or, Some(LogicalOperator::And))
                    );

                    if requires_parenthesis {
                        write!(f, "(")?;
                    }
                    for (i, spec) in group.iter().enumerate() {
                        if i > 0 {
                            write!(f, "{op}")?;
                        }
                        write(spec, f, Some(*op))?;
                    }
                    if requires_parenthesis {
                        write!(f, ")")?;
                    }
                    Ok(())
                }
                VersionSpec::None => write!(f, "!"),
            }
        }

        write(self, f, None)
    }
}

impl Serialize for VersionSpec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("{self}"))
    }
}

impl VersionSpec {
    /// Returns whether the version matches the specification.
    pub fn matches(&self, version: &Version) -> bool {
        match self {
            VersionSpec::None => false,
            VersionSpec::Any => true,
            VersionSpec::Exact(EqualityOperator::Equals, limit) => limit == version,
            VersionSpec::Exact(EqualityOperator::NotEquals, limit) => limit != version,
            VersionSpec::Range(RangeOperator::Greater, limit) => version > limit,
            VersionSpec::Range(RangeOperator::GreaterEquals, limit) => version >= limit,
            VersionSpec::Range(RangeOperator::Less, limit) => version < limit,
            VersionSpec::Range(RangeOperator::LessEquals, limit) => version <= limit,
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
    use std::str::FromStr;

    use assert_matches::assert_matches;
    use rstest::rstest;

    use crate::{
        version_spec::{
            parse::ParseConstraintError, EqualityOperator, LogicalOperator, ParseVersionSpecError,
            RangeOperator,
        },
        ParseStrictness, Version, VersionSpec,
    };

    #[test]
    fn test_simple() {
        assert_eq!(
            VersionSpec::from_str("1.2.3", ParseStrictness::Strict),
            Ok(VersionSpec::Exact(
                EqualityOperator::Equals,
                Version::from_str("1.2.3").unwrap(),
            ))
        );
        assert_eq!(
            VersionSpec::from_str(">=1.2.3", ParseStrictness::Strict),
            Ok(VersionSpec::Range(
                RangeOperator::GreaterEquals,
                Version::from_str("1.2.3").unwrap(),
            ))
        );
    }

    #[test]
    fn test_group() {
        assert_eq!(
            VersionSpec::from_str(">=1.2.3,<2.0.0", ParseStrictness::Strict),
            Ok(VersionSpec::Group(
                LogicalOperator::And,
                vec![
                    VersionSpec::Range(
                        RangeOperator::GreaterEquals,
                        Version::from_str("1.2.3").unwrap(),
                    ),
                    VersionSpec::Range(RangeOperator::Less, Version::from_str("2.0.0").unwrap()),
                ],
            ))
        );
        assert_eq!(
            VersionSpec::from_str(">=1.2.3|<1.0.0", ParseStrictness::Strict),
            Ok(VersionSpec::Group(
                LogicalOperator::Or,
                vec![
                    VersionSpec::Range(
                        RangeOperator::GreaterEquals,
                        Version::from_str("1.2.3").unwrap(),
                    ),
                    VersionSpec::Range(RangeOperator::Less, Version::from_str("1.0.0").unwrap()),
                ],
            ))
        );
        assert_eq!(
            VersionSpec::from_str("((>=1.2.3)|<1.0.0)", ParseStrictness::Strict),
            Ok(VersionSpec::Group(
                LogicalOperator::Or,
                vec![
                    VersionSpec::Range(
                        RangeOperator::GreaterEquals,
                        Version::from_str("1.2.3").unwrap(),
                    ),
                    VersionSpec::Range(RangeOperator::Less, Version::from_str("1.0.0").unwrap()),
                ],
            ))
        );
    }

    #[test]
    fn test_matches() {
        let v1 = Version::from_str("1.2.0").unwrap();
        let vs1 = VersionSpec::from_str(">=1.2.3,<2.0.0", ParseStrictness::Strict).unwrap();
        assert!(!vs1.matches(&v1));

        let vs2 = VersionSpec::from_str("1.2", ParseStrictness::Strict).unwrap();
        assert!(vs2.matches(&v1));

        let v2 = Version::from_str("1.2.3").unwrap();
        assert!(vs1.matches(&v2));
        assert!(!vs2.matches(&v2));

        let v3 = Version::from_str("1!1.2.3").unwrap();
        println!("{v3:?}");

        assert!(!vs1.matches(&v3));
        assert!(!vs2.matches(&v3));

        let vs3 = VersionSpec::from_str(">=1!1.2,<1!2", ParseStrictness::Strict).unwrap();
        assert!(vs3.matches(&v3));
    }

    #[test]
    fn issue_204() {
        assert!(VersionSpec::from_str(">=3.8<3.9", ParseStrictness::Strict).is_err());
    }

    #[rstest]
    #[case("2.38.*", true)]
    #[case("2.38.0.*", true)]
    #[case("2.38.0.1*", false)]
    #[case("2.38.0a.*", false)]
    fn issue_685(#[case] spec: &str, #[case] starts_with: bool) {
        let spec = VersionSpec::from_str(spec, ParseStrictness::Strict).unwrap();
        let version = &Version::from_str("2.38").unwrap();
        assert_eq!(spec.matches(version), starts_with);
    }

    #[test]
    fn issue_225() {
        let spec = VersionSpec::from_str("~=2.4", ParseStrictness::Strict).unwrap();
        assert!(!spec.matches(&Version::from_str("3.1").unwrap()));
        assert!(spec.matches(&Version::from_str("2.4").unwrap()));
        assert!(spec.matches(&Version::from_str("2.5").unwrap()));
        assert!(!spec.matches(&Version::from_str("2.1").unwrap()));
    }

    #[test]
    fn issue_235() {
        assert_eq!(
            VersionSpec::from_str(">2.10*", ParseStrictness::Lenient).unwrap(),
            VersionSpec::from_str(">=2.10", ParseStrictness::Strict).unwrap()
        );
    }

    #[test]
    fn issue_mkl_double() {
        assert_eq!(
            VersionSpec::from_str("2023.*.*", ParseStrictness::Lenient).unwrap(),
            VersionSpec::from_str("2023.*", ParseStrictness::Lenient).unwrap()
        );
        assert!(VersionSpec::from_str("2023.*.*", ParseStrictness::Strict).is_err());
        assert_matches!(
            VersionSpec::from_str("2023.*.0", ParseStrictness::Lenient).unwrap_err(),
            ParseVersionSpecError::InvalidConstraint(
                ParseConstraintError::RegexConstraintsNotSupported
            )
        );
    }

    #[test]
    fn issue_722() {
        assert_eq!(
            VersionSpec::from_str("0.2.18.*.", ParseStrictness::Lenient).unwrap(),
            VersionSpec::from_str("0.2.18.*", ParseStrictness::Lenient).unwrap()
        );

        assert!(VersionSpec::from_str("0.2.18.*.", ParseStrictness::Strict).is_err());
    }

    #[test]
    fn issue_bracket_printing() {
        let v = VersionSpec::from_str("(>=1,<2)|>3", ParseStrictness::Lenient).unwrap();
        assert_eq!(format!("{v}"), ">=1,<2|>3");

        let v = VersionSpec::from_str("(>=1|<2),>3", ParseStrictness::Lenient).unwrap();
        assert_eq!(format!("{v}"), "(>=1|<2),>3");

        let v = VersionSpec::from_str("(>=1|<2)|>3", ParseStrictness::Lenient).unwrap();
        assert_eq!(format!("{v}"), ">=1|<2|>3");

        let v = VersionSpec::from_str("(>=1,<2),>3", ParseStrictness::Lenient).unwrap();
        assert_eq!(format!("{v}"), ">=1,<2,>3");

        let v =
            VersionSpec::from_str("((>=1|>2),(>3|>4))|(>5,<6)", ParseStrictness::Lenient).unwrap();
        assert_eq!(format!("{v}"), "(>=1|>2),(>3|>4)|>5,<6");
    }

    #[test]
    fn issue_star_operator() {
        assert_eq!(
            VersionSpec::from_str(">=*", ParseStrictness::Lenient).unwrap(),
            VersionSpec::from_str("*", ParseStrictness::Lenient).unwrap()
        );
        assert_eq!(
            VersionSpec::from_str("==*", ParseStrictness::Lenient).unwrap(),
            VersionSpec::from_str("*", ParseStrictness::Lenient).unwrap()
        );
        assert_eq!(
            VersionSpec::from_str("=*", ParseStrictness::Lenient).unwrap(),
            VersionSpec::from_str("*", ParseStrictness::Lenient).unwrap()
        );
        assert_eq!(
            VersionSpec::from_str("~=*", ParseStrictness::Lenient).unwrap(),
            VersionSpec::from_str("*", ParseStrictness::Lenient).unwrap()
        );
        assert_eq!(
            VersionSpec::from_str("<=*", ParseStrictness::Lenient).unwrap(),
            VersionSpec::from_str("*", ParseStrictness::Lenient).unwrap()
        );

        assert_matches!(
            VersionSpec::from_str(">*", ParseStrictness::Lenient).unwrap_err(),
            ParseVersionSpecError::InvalidConstraint(
                ParseConstraintError::GlobVersionIncompatibleWithOperator(_)
            )
        );
        assert_matches!(
            VersionSpec::from_str("!=*", ParseStrictness::Lenient).unwrap_err(),
            ParseVersionSpecError::InvalidConstraint(
                ParseConstraintError::GlobVersionIncompatibleWithOperator(_)
            )
        );
        assert_matches!(
            VersionSpec::from_str("<*", ParseStrictness::Lenient).unwrap_err(),
            ParseVersionSpecError::InvalidConstraint(
                ParseConstraintError::GlobVersionIncompatibleWithOperator(_)
            )
        );

        assert_matches!(
            VersionSpec::from_str(">=*", ParseStrictness::Strict).unwrap_err(),
            ParseVersionSpecError::InvalidConstraint(
                ParseConstraintError::GlobVersionIncompatibleWithOperator(_)
            )
        );
        assert_matches!(
            VersionSpec::from_str("==*", ParseStrictness::Strict).unwrap_err(),
            ParseVersionSpecError::InvalidConstraint(
                ParseConstraintError::GlobVersionIncompatibleWithOperator(_)
            )
        );
        assert_matches!(
            VersionSpec::from_str("=*", ParseStrictness::Strict).unwrap_err(),
            ParseVersionSpecError::InvalidConstraint(
                ParseConstraintError::GlobVersionIncompatibleWithOperator(_)
            )
        );
        assert_matches!(
            VersionSpec::from_str("~=*", ParseStrictness::Strict).unwrap_err(),
            ParseVersionSpecError::InvalidConstraint(
                ParseConstraintError::GlobVersionIncompatibleWithOperator(_)
            )
        );
        assert_matches!(
            VersionSpec::from_str("<=*", ParseStrictness::Strict).unwrap_err(),
            ParseVersionSpecError::InvalidConstraint(
                ParseConstraintError::GlobVersionIncompatibleWithOperator(_)
            )
        );
    }
}
