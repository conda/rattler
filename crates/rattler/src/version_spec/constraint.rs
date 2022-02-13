use super::VersionOperator;
use crate::{ParseVersionError, Version};
use std::str::FromStr;
use thiserror::Error;

/// A single version constraint (e.g. `>3.4.5` or `1.2.*`)
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum Constraint {
    /// Matches anything (`*`)
    Any,

    /// Version comparison (e.g `>1.2.3`)
    Comparison(VersionOperator, Version),
}

#[derive(Debug, Clone, Error, Eq, PartialEq)]
pub enum ParseConstraintError {
    #[error("cannot parse version: {0}")]
    InvalidVersion(#[source] ParseVersionError),
    #[error("version operator followed by a whitespace")]
    OperatorFollowedByWhitespace,
    #[error("'.' is incompatible with '{0}' operator'")]
    GlobVersionIncompatibleWithOperator(VersionOperator),
    #[error("regex constraints are not supported")]
    RegexConstraintsNotSupported,
    #[error("invalid operator")]
    InvalidOperator,
}

/// Parses an operator from a string. Returns the operator and the rest of the string.
fn parse_operator(s: &str) -> Option<(VersionOperator, &str)> {
    if let Some(rest) = s.strip_prefix("==") {
        Some((VersionOperator::Equals, rest))
    } else if let Some(rest) = s.strip_prefix("!=") {
        Some((VersionOperator::NotEquals, rest))
    } else if let Some(rest) = s.strip_prefix("<=") {
        Some((VersionOperator::LessEquals, rest))
    } else if let Some(rest) = s.strip_prefix(">=") {
        Some((VersionOperator::GreaterEquals, rest))
    } else if let Some(rest) = s.strip_prefix("~=") {
        Some((VersionOperator::Compatible, rest))
    } else if let Some(rest) = s.strip_prefix("<") {
        Some((VersionOperator::Less, rest))
    } else if let Some(rest) = s.strip_prefix(">") {
        Some((VersionOperator::Greater, rest))
    } else if let Some(rest) = s.strip_prefix("=") {
        Some((VersionOperator::StartsWith, rest))
    } else if s.starts_with(|c: char| c.is_alphanumeric()) {
        Some((VersionOperator::Equals, s))
    } else {
        None
    }
}

/// Returns true if the specified character is the first character of a version constraint.
pub(crate) fn is_start_of_version_constraint(c: char) -> bool {
    matches!(c, '>' | '<' | '=' | '!' | '~')
}

impl FromStr for Constraint {
    type Err = ParseConstraintError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s == "*" {
            Ok(Constraint::Any)
        } else if s.starts_with("^") || s.ends_with("$") {
            Err(ParseConstraintError::RegexConstraintsNotSupported)
        } else if s.starts_with(is_start_of_version_constraint) {
            let (op, version_str) =
                parse_operator(s).ok_or(ParseConstraintError::InvalidOperator)?;
            if !version_str.starts_with(char::is_alphanumeric) {
                return Err(ParseConstraintError::InvalidOperator);
            }
            if version_str.starts_with(char::is_whitespace) {
                return Err(ParseConstraintError::OperatorFollowedByWhitespace);
            }
            let (version_str, op) = if let Some(version_str) = version_str.strip_suffix(".*") {
                match op {
                    VersionOperator::StartsWith | VersionOperator::GreaterEquals => {
                        (version_str, op)
                    }
                    VersionOperator::NotEquals => (version_str, VersionOperator::NotStartsWith),
                    op => {
                        return Err(ParseConstraintError::GlobVersionIncompatibleWithOperator(
                            op,
                        ))
                    }
                }
            } else {
                (version_str, op)
            };
            Ok(Constraint::Comparison(
                op,
                Version::from_str(version_str).map_err(ParseConstraintError::InvalidVersion)?,
            ))
        } else if s.ends_with("*") {
            let version_str = s.trim_end_matches('*').trim_end_matches('.');
            Ok(Constraint::Comparison(
                VersionOperator::StartsWith,
                Version::from_str(version_str).map_err(ParseConstraintError::InvalidVersion)?,
            ))
        } else if s.contains('*') {
            Err(ParseConstraintError::RegexConstraintsNotSupported)
        } else {
            Ok(Constraint::Comparison(
                VersionOperator::Equals,
                Version::from_str(s).map_err(ParseConstraintError::InvalidVersion)?,
            ))
        }
    }
}

#[cfg(test)]
mod test {
    use super::Constraint;
    use crate::version_spec::constraint::ParseConstraintError;
    use crate::version_spec::VersionOperator;
    use crate::Version;
    use std::str::FromStr;

    #[test]
    fn test_empty() {
        assert!(matches!(
            Constraint::from_str(""),
            Err(ParseConstraintError::InvalidVersion(_))
        ));
    }

    #[test]
    fn test_any() {
        assert_eq!(Constraint::from_str("*"), Ok(Constraint::Any));
    }

    #[test]
    fn test_invalid_op() {
        assert_eq!(
            Constraint::from_str("<>1.2.3"),
            Err(ParseConstraintError::InvalidOperator)
        );
        assert_eq!(
            Constraint::from_str("=!1.2.3"),
            Err(ParseConstraintError::InvalidOperator)
        );
        assert_eq!(
            Constraint::from_str("<!=1.2.3"),
            Err(ParseConstraintError::InvalidOperator)
        );
        assert_eq!(
            Constraint::from_str("<!>1.2.3"),
            Err(ParseConstraintError::InvalidOperator)
        );
        assert_eq!(
            Constraint::from_str("!=!1.2.3"),
            Err(ParseConstraintError::InvalidOperator)
        );
        assert_eq!(
            Constraint::from_str("<=>1.2.3"),
            Err(ParseConstraintError::InvalidOperator)
        );
    }

    #[test]
    fn test_op() {
        assert_eq!(
            Constraint::from_str(">1.2.3"),
            Ok(Constraint::Comparison(
                VersionOperator::Greater,
                Version::from_str("1.2.3").unwrap()
            ))
        );
        assert_eq!(
            Constraint::from_str("<1.2.3"),
            Ok(Constraint::Comparison(
                VersionOperator::Less,
                Version::from_str("1.2.3").unwrap()
            ))
        );
        assert_eq!(
            Constraint::from_str("=1.2.3"),
            Ok(Constraint::Comparison(
                VersionOperator::StartsWith,
                Version::from_str("1.2.3").unwrap()
            ))
        );
        assert_eq!(
            Constraint::from_str("==1.2.3"),
            Ok(Constraint::Comparison(
                VersionOperator::Equals,
                Version::from_str("1.2.3").unwrap()
            ))
        );
        assert_eq!(
            Constraint::from_str("!=1.2.3"),
            Ok(Constraint::Comparison(
                VersionOperator::NotEquals,
                Version::from_str("1.2.3").unwrap()
            ))
        );
        assert_eq!(
            Constraint::from_str("~=1.2.3"),
            Ok(Constraint::Comparison(
                VersionOperator::Compatible,
                Version::from_str("1.2.3").unwrap()
            ))
        );
        assert_eq!(
            Constraint::from_str(">=1.2.3"),
            Ok(Constraint::Comparison(
                VersionOperator::GreaterEquals,
                Version::from_str("1.2.3").unwrap()
            ))
        );
        assert_eq!(
            Constraint::from_str("<=1.2.3"),
            Ok(Constraint::Comparison(
                VersionOperator::LessEquals,
                Version::from_str("1.2.3").unwrap()
            ))
        );
    }

    #[test]
    fn test_glob_op() {
        assert_eq!(
            Constraint::from_str("=1.2.*"),
            Ok(Constraint::Comparison(
                VersionOperator::StartsWith,
                Version::from_str("1.2").unwrap()
            ))
        );
        assert_eq!(
            Constraint::from_str("!=1.2.*"),
            Ok(Constraint::Comparison(
                VersionOperator::NotStartsWith,
                Version::from_str("1.2").unwrap()
            ))
        );
        assert_eq!(
            Constraint::from_str(">=1.2.*"),
            Ok(Constraint::Comparison(
                VersionOperator::GreaterEquals,
                Version::from_str("1.2").unwrap()
            ))
        );
        assert_eq!(
            Constraint::from_str("==1.2.*"),
            Err(ParseConstraintError::GlobVersionIncompatibleWithOperator(
                VersionOperator::Equals
            ))
        );
        assert_eq!(
            Constraint::from_str(">1.2.*"),
            Err(ParseConstraintError::GlobVersionIncompatibleWithOperator(
                VersionOperator::Greater
            ))
        );
        assert_eq!(
            Constraint::from_str("<=1.2.*"),
            Err(ParseConstraintError::GlobVersionIncompatibleWithOperator(
                VersionOperator::LessEquals
            ))
        );
        assert_eq!(
            Constraint::from_str("<1.2.*"),
            Err(ParseConstraintError::GlobVersionIncompatibleWithOperator(
                VersionOperator::Less
            ))
        );
    }

    #[test]
    fn test_starts_with() {
        assert_eq!(
            Constraint::from_str("1.2.*"),
            Ok(Constraint::Comparison(
                VersionOperator::StartsWith,
                Version::from_str("1.2").unwrap()
            ))
        );
        assert!(matches!(
            Constraint::from_str("1.2.*.*"),
            Err(ParseConstraintError::InvalidVersion(_))
        ));
    }

    #[test]
    fn test_exact() {
        assert_eq!(
            Constraint::from_str("1.2.3"),
            Ok(Constraint::Comparison(
                VersionOperator::Equals,
                Version::from_str("1.2.3").unwrap()
            ))
        );
    }

    #[test]
    fn test_regex() {
        assert_eq!(
            Constraint::from_str("^1.2.3"),
            Err(ParseConstraintError::RegexConstraintsNotSupported)
        );
        assert_eq!(
            Constraint::from_str("1.2.3$"),
            Err(ParseConstraintError::RegexConstraintsNotSupported)
        );
        assert_eq!(
            Constraint::from_str("1.*.3"),
            Err(ParseConstraintError::RegexConstraintsNotSupported)
        );
    }
}
