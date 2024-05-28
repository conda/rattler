use super::ParseConstraintError;
use super::RangeOperator;
use crate::version_spec::parse::constraint_parser;
use crate::version_spec::{EqualityOperator, StrictRangeOperator};
use crate::{ParseStrictness, Version};
use std::str::FromStr;

/// A single version constraint (e.g. `>3.4.5` or `1.2.*`)
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum Constraint {
    /// Matches anything (`*`)
    Any,

    /// Version comparison (e.g `>1.2.3`)
    Comparison(RangeOperator, Version),

    /// Strict comparison (e.g `~=1.2.3`)
    StrictComparison(StrictRangeOperator, Version),

    /// Exact Version
    Exact(EqualityOperator, Version),
}

/// Returns true if the specified character is the first character of a version constraint.
pub(crate) fn is_start_of_version_constraint(c: char) -> bool {
    matches!(c, '>' | '<' | '=' | '!' | '~')
}

impl FromStr for Constraint {
    type Err = ParseConstraintError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Constraint::from_str(s, ParseStrictness::Lenient)
    }
}

impl Constraint {
    pub fn from_str(
        input: &str,
        strictness: ParseStrictness,
    ) -> Result<Self, ParseConstraintError> {
        match constraint_parser(strictness)(input) {
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
    use crate::version_spec::{EqualityOperator, RangeOperator, StrictRangeOperator};
    use crate::{ParseStrictness, ParseStrictness::*, Version};
    use assert_matches::assert_matches;
    use rstest::rstest;
    use std::str::FromStr;

    #[rstest]
    fn test_empty(#[values(Lenient, Strict)] strictness: ParseStrictness) {
        assert!(matches!(
            Constraint::from_str("", strictness),
            Err(ParseConstraintError::InvalidVersion(_))
        ));
    }

    #[test]
    fn test_any() {
        assert_eq!(Constraint::from_str("*", Lenient), Ok(Constraint::Any));
        assert_eq!(Constraint::from_str("*", Strict), Ok(Constraint::Any));
        assert_eq!(Constraint::from_str("*.*", Lenient), Ok(Constraint::Any));
        assert_eq!(
            Constraint::from_str("*.*", Strict),
            Err(ParseConstraintError::InvalidGlob)
        );
    }

    #[rstest]
    fn test_invalid_op(#[values(Lenient, Strict)] strictness: ParseStrictness) {
        assert_eq!(
            Constraint::from_str("<>1.2.3", strictness),
            Err(ParseConstraintError::InvalidOperator(String::from("<>")))
        );
        assert_eq!(
            Constraint::from_str("=!1.2.3", strictness),
            Err(ParseConstraintError::InvalidOperator(String::from("=!")))
        );
        assert_eq!(
            Constraint::from_str("<!=1.2.3", strictness),
            Err(ParseConstraintError::InvalidOperator(String::from("<!=")))
        );
        assert_eq!(
            Constraint::from_str("<!>1.2.3", strictness),
            Err(ParseConstraintError::InvalidOperator(String::from("<!>")))
        );
        assert_eq!(
            Constraint::from_str("!=!1.2.3", strictness),
            Err(ParseConstraintError::InvalidOperator(String::from("!=!")))
        );
        assert_eq!(
            Constraint::from_str("<=>1.2.3", strictness),
            Err(ParseConstraintError::InvalidOperator(String::from("<=>")))
        );
        assert_eq!(
            Constraint::from_str("=>1.2.3", strictness),
            Err(ParseConstraintError::InvalidOperator(String::from("=>")))
        );
    }

    #[rstest]
    fn test_op(#[values(Lenient, Strict)] strictness: ParseStrictness) {
        assert_eq!(
            Constraint::from_str(">1.2.3", strictness),
            Ok(Constraint::Comparison(
                RangeOperator::Greater,
                Version::from_str("1.2.3").unwrap(),
            ))
        );
        assert_eq!(
            Constraint::from_str("<1.2.3", strictness),
            Ok(Constraint::Comparison(
                RangeOperator::Less,
                Version::from_str("1.2.3").unwrap(),
            ))
        );
        assert_eq!(
            Constraint::from_str("=1.2.3", strictness),
            Ok(Constraint::StrictComparison(
                StrictRangeOperator::StartsWith,
                Version::from_str("1.2.3").unwrap(),
            ))
        );
        assert_eq!(
            Constraint::from_str("==1.2.3", strictness),
            Ok(Constraint::Exact(
                EqualityOperator::Equals,
                Version::from_str("1.2.3").unwrap(),
            ))
        );
        assert_eq!(
            Constraint::from_str("!=1.2.3", strictness),
            Ok(Constraint::Exact(
                EqualityOperator::NotEquals,
                Version::from_str("1.2.3").unwrap(),
            ))
        );
        assert_eq!(
            Constraint::from_str("~=1.2.3", strictness),
            Ok(Constraint::StrictComparison(
                StrictRangeOperator::Compatible,
                Version::from_str("1.2.3").unwrap(),
            ))
        );
        assert_eq!(
            Constraint::from_str(">=1.2.3", strictness),
            Ok(Constraint::Comparison(
                RangeOperator::GreaterEquals,
                Version::from_str("1.2.3").unwrap(),
            ))
        );
        assert_eq!(
            Constraint::from_str("<=1.2.3", strictness),
            Ok(Constraint::Comparison(
                RangeOperator::LessEquals,
                Version::from_str("1.2.3").unwrap(),
            ))
        );
        assert_eq!(
            Constraint::from_str(">=1!1.2", strictness),
            Ok(Constraint::Comparison(
                RangeOperator::GreaterEquals,
                Version::from_str("1!1.2").unwrap(),
            ))
        );
    }

    #[test]
    fn test_glob_op_lenient() {
        assert_eq!(
            Constraint::from_str("=1.2.*", Lenient),
            Ok(Constraint::StrictComparison(
                StrictRangeOperator::StartsWith,
                Version::from_str("1.2").unwrap(),
            ))
        );
        assert_eq!(
            Constraint::from_str("!=1.2.*", Lenient),
            Ok(Constraint::StrictComparison(
                StrictRangeOperator::NotStartsWith,
                Version::from_str("1.2").unwrap(),
            ))
        );
        assert_eq!(
            Constraint::from_str(">=1.2.*", Lenient),
            Ok(Constraint::Comparison(
                RangeOperator::GreaterEquals,
                Version::from_str("1.2").unwrap(),
            ))
        );
        assert_eq!(
            Constraint::from_str("==1.2.*", Lenient),
            Ok(Constraint::Exact(
                EqualityOperator::Equals,
                Version::from_str("1.2").unwrap(),
            ))
        );
        assert_eq!(
            Constraint::from_str(">1.2.*", Lenient),
            Ok(Constraint::Comparison(
                RangeOperator::GreaterEquals,
                Version::from_str("1.2").unwrap(),
            ))
        );
        assert_eq!(
            Constraint::from_str("<=1.2.*", Lenient),
            Ok(Constraint::Comparison(
                RangeOperator::LessEquals,
                Version::from_str("1.2").unwrap(),
            ))
        );
        assert_eq!(
            Constraint::from_str("<1.2.*", Lenient),
            Ok(Constraint::Comparison(
                RangeOperator::Less,
                Version::from_str("1.2").unwrap(),
            ))
        );
    }

    #[test]
    fn test_glob_op_strict() {
        assert_matches!(
            Constraint::from_str("=1.2.*", Strict),
            Err(ParseConstraintError::GlobVersionIncompatibleWithOperator(_))
        );
        assert_eq!(
            Constraint::from_str("!=1.2.*", Lenient),
            Ok(Constraint::StrictComparison(
                StrictRangeOperator::NotStartsWith,
                Version::from_str("1.2").unwrap(),
            ))
        );
        assert_matches!(
            Constraint::from_str(">=1.2.*", Strict),
            Err(ParseConstraintError::GlobVersionIncompatibleWithOperator(_))
        );
        assert_matches!(
            Constraint::from_str("==1.2.*", Strict),
            Err(ParseConstraintError::GlobVersionIncompatibleWithOperator(_))
        );
        assert_matches!(
            Constraint::from_str(">1.2.*", Strict),
            Err(ParseConstraintError::GlobVersionIncompatibleWithOperator(_))
        );
        assert_matches!(
            Constraint::from_str("<=1.2.*", Strict),
            Err(ParseConstraintError::GlobVersionIncompatibleWithOperator(_))
        );
        assert_matches!(
            Constraint::from_str("<1.2.*", Strict),
            Err(ParseConstraintError::GlobVersionIncompatibleWithOperator(_))
        );
    }

    #[rstest]
    fn test_starts_with(#[values(Lenient, Strict)] strictness: ParseStrictness) {
        assert_eq!(
            Constraint::from_str("1.2.*", strictness),
            Ok(Constraint::StrictComparison(
                StrictRangeOperator::StartsWith,
                Version::from_str("1.2").unwrap(),
            ))
        );
    }

    #[rstest]
    fn test_exact(#[values(Lenient, Strict)] strictness: ParseStrictness) {
        assert_eq!(
            Constraint::from_str("1.2.3", strictness),
            Ok(Constraint::Exact(
                EqualityOperator::Equals,
                Version::from_str("1.2.3").unwrap(),
            ))
        );
    }

    #[rstest]
    fn test_regex(#[values(Lenient, Strict)] strictness: ParseStrictness) {
        assert_eq!(
            Constraint::from_str("^1.2.3", strictness),
            Err(ParseConstraintError::UnterminatedRegex)
        );
        assert_eq!(
            Constraint::from_str("1.2.3$", strictness),
            Err(ParseConstraintError::UnterminatedRegex)
        );
        assert_eq!(
            Constraint::from_str("1.*.3", strictness),
            Err(ParseConstraintError::RegexConstraintsNotSupported)
        );
    }
}
