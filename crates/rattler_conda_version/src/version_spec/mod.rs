//! Version-matching constraints from conda `MatchSpecs`, as specified by
//! [CEP 29](https://conda.org/learn/ceps/cep-0029).
//!
//! [`VersionSpec`] represents the version portion of a `MatchSpec`, such as
//! `>=3.4,<4.0`. It supports equality, ranges, prefix matching, compatible
//! matching, and groups joined with `,` (and) or `|` (or).

mod constraint;
pub(crate) mod parse;

use std::{
    borrow::Cow,
    fmt::{Display, Formatter},
    str::FromStr,
};

use constraint::Constraint;
pub use parse::ParseConstraintError;
use serde::{Deserialize, Serialize, Serializer};
use thiserror::Error;

use crate::{
    ParseStrictness,
    ParseStrictness::Lenient,
    version::{ParseVersionError, StrictVersion, Version},
};

/// A relational comparison operator in a [`VersionSpec`] range clause.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum RangeOperator {
    /// Matches versions greater than the bound (`>`).
    Greater,
    /// Matches versions greater than or equal to the bound (`>=`).
    GreaterEquals,
    /// Matches versions less than the bound (`<`).
    Less,
    /// Matches versions less than or equal to the bound (`<=`).
    LessEquals,
}

impl RangeOperator {
    /// Returns the relational operator that negates this comparison.
    pub fn complement(self) -> Self {
        match self {
            RangeOperator::Greater => RangeOperator::LessEquals,
            RangeOperator::GreaterEquals => RangeOperator::Less,
            RangeOperator::Less => RangeOperator::GreaterEquals,
            RangeOperator::LessEquals => RangeOperator::Greater,
        }
    }
}

/// A conda-specific comparison operator in a [`VersionSpec`] strict-range clause.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum StrictRangeOperator {
    /// Matches versions beginning with the bound (`=` or a trailing `.*`).
    StartsWith,
    /// Excludes versions beginning with the bound (`!=...*`).
    NotStartsWith,
    /// Matches versions compatible with the bound (`~=`).
    Compatible,
    /// Excludes versions compatible with the bound (`!~=`).
    NotCompatible,
}

impl StrictRangeOperator {
    /// Returns the strict-range operator that negates this comparison.
    pub fn complement(self) -> Self {
        match self {
            StrictRangeOperator::StartsWith => StrictRangeOperator::NotStartsWith,
            StrictRangeOperator::NotStartsWith => StrictRangeOperator::StartsWith,
            StrictRangeOperator::Compatible => StrictRangeOperator::NotCompatible,
            StrictRangeOperator::NotCompatible => StrictRangeOperator::Compatible,
        }
    }
}

/// An equality comparison operator in a [`VersionSpec`] exact clause.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum EqualityOperator {
    /// Matches the exact version (`==`).
    Equals,
    /// Excludes the exact version (`!=`).
    NotEquals,
}

impl EqualityOperator {
    /// Returns the equality operator that negates this comparison.
    pub fn complement(self) -> Self {
        match self {
            EqualityOperator::Equals => EqualityOperator::NotEquals,
            EqualityOperator::NotEquals => EqualityOperator::Equals,
        }
    }
}

/// The comparison operator parsed from one conda version-constraint clause.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize)]
pub enum VersionOperators {
    /// A standard relational range operator.
    Range(RangeOperator),
    /// A conda-specific strict range operator.
    StrictRange(StrictRangeOperator),
    /// An equality operator.
    Exact(EqualityOperator),
}

/// Connects child constraints in a grouped [`VersionSpec`].
///
/// `,` requires every constraint to match; `|` requires at least one.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum LogicalOperator {
    /// Requires every constraint in the group to match (written as `,`).
    And,

    /// Requires at least one constraint in the group to match (written as `|`).
    Or,
}

impl LogicalOperator {
    /// Returns the logical operator that negates this connector.
    pub fn complement(self) -> Self {
        match self {
            LogicalOperator::And => LogicalOperator::Or,
            LogicalOperator::Or => LogicalOperator::And,
        }
    }
}

/// A parsed conda version constraint.
///
/// This is the version-matching portion of a conda `MatchSpec`, specified by
/// [CEP 29](https://conda.org/learn/ceps/cep-0029).
///
/// ```
/// # use rattler_conda_version::{ParseStrictness, Version, VersionSpec};
/// # use std::str::FromStr;
/// let spec = VersionSpec::from_str(">=1.2,<2", ParseStrictness::Lenient).unwrap();
/// assert!(spec.matches(&Version::from_str("1.5").unwrap()));
/// assert!(!spec.matches(&Version::from_str("2.0").unwrap()));
/// ```
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum VersionSpec {
    /// An explicitly absent version constraint (`!`).
    None,
    /// Any version (`*`).
    Any,
    /// A relational comparison against a version.
    Range(RangeOperator, Version),
    /// A conda-specific strict comparison against a version.
    StrictRange(StrictRangeOperator, StrictVersion),
    /// An equality comparison against a version.
    Exact(EqualityOperator, Version),
    /// A group joined by a logical operator.
    Group(LogicalOperator, Vec<VersionSpec>),
}

/// An error while parsing a complete conda version specification.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Eq, PartialEq, Error)]
pub enum ParseVersionSpecError {
    /// A version inside the specification is invalid.
    #[error(transparent)]
    InvalidVersion(#[from] ParseVersionError),

    /// The constraint syntax is invalid.
    #[error(transparent)]
    InvalidConstraint(#[from] ParseConstraintError),
}

impl From<Constraint> for VersionSpec {
    fn from(constraint: Constraint) -> Self {
        match constraint {
            Constraint::Any => VersionSpec::Any,
            Constraint::Comparison(op, ver) => VersionSpec::Range(op, ver),
            Constraint::StrictComparison(op, ver) => {
                VersionSpec::StrictRange(op, StrictVersion::from(ver))
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
    /// Parses the version expression from a conda match spec with the requested strictness.
    ///
    /// Use [`ParseStrictness::Strict`] for newly authored specifications and
    /// [`ParseStrictness::Lenient`] for existing user input or metadata.
    pub fn from_str(
        source: &str,
        strictness: ParseStrictness,
    ) -> Result<Self, ParseVersionSpecError> {
        parse::version_spec_parser(source, strictness)
            .map_err(ParseVersionSpecError::InvalidConstraint)
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
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for VersionSpec {
    fn deserialize<D>(deserializer: D) -> Result<VersionSpec, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        VersionSpec::from_str(&s, Lenient).map_err(serde::de::Error::custom)
    }
}

impl VersionSpec {
    /// Evaluates this conda version constraint against `version`.
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
                version.starts_with(limit)
            }
            VersionSpec::StrictRange(StrictRangeOperator::NotStartsWith, limit) => {
                !version.starts_with(limit)
            }
            VersionSpec::StrictRange(StrictRangeOperator::Compatible, limit) => {
                version.compatible_with(limit)
            }
            VersionSpec::StrictRange(StrictRangeOperator::NotCompatible, limit) => {
                !version.compatible_with(limit)
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
        ParseStrictness, Version, VersionSpec,
        version::StrictVersion,
        version_spec::{
            EqualityOperator, LogicalOperator, ParseVersionSpecError, RangeOperator,
            StrictRangeOperator, parse::ParseConstraintError,
        },
    };
    use EqualityOperator::Equals;
    use LogicalOperator::{And, Or};
    use RangeOperator::LessEquals;

    #[test]
    fn test_simple() {
        assert_eq!(
            VersionSpec::from_str("==1.2.3", ParseStrictness::Strict),
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
        assert_eq!(
            VersionSpec::from_str("=1.2.3", ParseStrictness::Strict),
            Ok(VersionSpec::StrictRange(
                StrictRangeOperator::StartsWith,
                StrictVersion::from_str("1.2.3").unwrap(),
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
    fn test_group_flattening() {
        let exact = |v: &str| VersionSpec::Exact(Equals, Version::from_str(v).unwrap());
        let le = |v: &str| VersionSpec::Range(LessEquals, Version::from_str(v).unwrap());

        // A single-element parenthesized group is unwrapped, not nested.
        assert_eq!(
            VersionSpec::from_str("1.2.3,(4.5.6),<=7.8.9", ParseStrictness::Lenient),
            Ok(VersionSpec::Group(
                And,
                vec![exact("1.2.3"), exact("4.5.6"), le("7.8.9")]
            ))
        );

        // Nested `Or` groups of the same operator are flattened into one.
        assert_eq!(
            VersionSpec::from_str("((1.2.3)|(4.5.6))|<=7.8.9", ParseStrictness::Lenient),
            Ok(VersionSpec::Group(
                Or,
                vec![exact("1.2.3"), exact("4.5.6"), le("7.8.9")]
            ))
        );

        // `,` binds tighter than `|`, producing a nested `And` inside an `Or`.
        assert_eq!(
            VersionSpec::from_str("1.2.3,4.5.6|<=7.8.9", ParseStrictness::Lenient),
            Ok(VersionSpec::Group(
                Or,
                vec![
                    VersionSpec::Group(And, vec![exact("1.2.3"), exact("4.5.6")]),
                    le("7.8.9"),
                ]
            ))
        );

        // Redundant parentheses collapse to a single constraint.
        assert_eq!(
            VersionSpec::from_str("((((1.5))))", ParseStrictness::Lenient),
            Ok(exact("1.5"))
        );
    }

    #[test]
    fn test_matches() {
        let v1 = Version::from_str("1.2.0").unwrap();
        let vs1 = VersionSpec::from_str(">=1.2.3,<2.0.0", ParseStrictness::Strict).unwrap();
        assert!(!vs1.matches(&v1));

        let vs2 = VersionSpec::from_str("==1.2.0", ParseStrictness::Strict).unwrap();
        assert!(vs2.matches(&v1));

        let v2 = Version::from_str("1.2.3").unwrap();
        assert!(vs1.matches(&v2));
        assert!(!vs2.matches(&v2));

        let v3 = Version::from_str("1!1.2.3").unwrap();

        assert!(!vs1.matches(&v3));
        assert!(!vs2.matches(&v3));

        let vs3 = VersionSpec::from_str(">=1!1.2,<1!2", ParseStrictness::Strict).unwrap();
        assert!(vs3.matches(&v3));

        let vs4 = VersionSpec::from_str("1!1.2.*", ParseStrictness::Strict).unwrap();
        assert!(vs4.matches(&v3));
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
    fn issue_1004() {
        assert_eq!(
            VersionSpec::from_str(">=2.*.*", ParseStrictness::Lenient).unwrap(),
            VersionSpec::from_str(">=2", ParseStrictness::Lenient).unwrap()
        );

        assert!(VersionSpec::from_str("0.2.18.*.*", ParseStrictness::Strict).is_err());
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
