use super::ParseConstraintError;
use super::VersionOperator;
use crate::version_spec::parse::constraint_parser;
use crate::Version;
use std::str::FromStr;

/// A single version constraint (e.g. `>3.4.5` or `1.2.*`)
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(crate) enum Constraint {
    /// Matches anything (`*`)
    Any,

    /// Version comparison (e.g `>1.2.3`)
    Comparison(VersionOperator, Version),
}

/// Returns true if the specified character is the first character of a version constraint.
pub(crate) fn is_start_of_version_constraint(c: char) -> bool {
    matches!(c, '>' | '<' | '=' | '!' | '~')
}

impl FromStr for Constraint {
    type Err = ParseConstraintError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match constraint_parser(input) {
            Ok(("", version)) => Ok(version),
            Ok((_, _)) => Err(ParseConstraintError::ExpectedEof),
            Err(nom::Err::Failure(e) | nom::Err::Error(e)) => Err(e),
            Err(_) => unreachable!("not streaming, so no other error possible"),
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
            Err(ParseConstraintError::InvalidOperator(String::from("<>")))
        );
        assert_eq!(
            Constraint::from_str("=!1.2.3"),
            Err(ParseConstraintError::InvalidOperator(String::from("=!")))
        );
        assert_eq!(
            Constraint::from_str("<!=1.2.3"),
            Err(ParseConstraintError::InvalidOperator(String::from("<!=")))
        );
        assert_eq!(
            Constraint::from_str("<!>1.2.3"),
            Err(ParseConstraintError::InvalidOperator(String::from("<!>")))
        );
        assert_eq!(
            Constraint::from_str("!=!1.2.3"),
            Err(ParseConstraintError::InvalidOperator(String::from("!=!")))
        );
        assert_eq!(
            Constraint::from_str("<=>1.2.3"),
            Err(ParseConstraintError::InvalidOperator(String::from("<=>")))
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
        assert_eq!(
            Constraint::from_str(">=1!1.2"),
            Ok(Constraint::Comparison(
                VersionOperator::GreaterEquals,
                Version::from_str("1!1.2").unwrap()
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
            Ok(Constraint::Comparison(
                VersionOperator::Equals,
                Version::from_str("1.2").unwrap()
            ))
        );
        assert_eq!(
            Constraint::from_str(">1.2.*"),
            Ok(Constraint::Comparison(
                VersionOperator::GreaterEquals,
                Version::from_str("1.2").unwrap()
            ))
        );
        assert_eq!(
            Constraint::from_str("<=1.2.*"),
            Ok(Constraint::Comparison(
                VersionOperator::LessEquals,
                Version::from_str("1.2").unwrap()
            ))
        );
        assert_eq!(
            Constraint::from_str("<1.2.*"),
            Ok(Constraint::Comparison(
                VersionOperator::Less,
                Version::from_str("1.2").unwrap()
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
        assert_eq!(
            Constraint::from_str("1.2.*.*"),
            Err(ParseConstraintError::RegexConstraintsNotSupported)
        );
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
            Err(ParseConstraintError::UnterminatedRegex)
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
