//! This module contains code to work with build specs in a MatchSpec
// use constraint::OrdConstraint;

mod parse;
use crate::constraint;
use std::fmt::{self, Display, Formatter};

type BuildNumber = u64;
/// named type for the Set specified by BuildNumberOperator on BuildNumber
type BuildNumberSpec = constraint::OperatorConstraint<BuildNumber, BuildNumberOperator>;

#[derive(Debug, Clone, PartialEq, Eq)]
enum BuildNumberOperator {
    Greater(constraint::Greater),
    Less(constraint::Less),
    Equal(constraint::Equal),
}

impl Display for BuildNumberOperator {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Greater(op) => op.fmt(f),
            Self::Less(op) => op.fmt(f),
            Self::Equal(op) => op.fmt(f),
        }
    }
}

impl<Element> constraint::Operator<Element> for BuildNumberOperator
where
    Element: std::cmp::PartialOrd,
{
    fn compares(&self, source: &Element, target: &Element) -> bool {
        match self {
            Self::Greater(op) => op.compares(&source, &target),
            Self::Less(op) => op.compares(&source, &target),
            Self::Equal(op) => op.compares(&source, &target),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::constraint::Set;

    use super::*;

    #[test]
    fn check_build_number_cmp_spec() {
        let above = 10;
        let below = 1;
        let exact = 5;
        let spec: BuildNumberSpec = (">=".to_string() + &exact.to_string()).parse().unwrap();

        assert!(!spec.is_member(&below), "{below} not ge {exact}");
        assert!(spec.is_member(&above), "{above} ge {exact}");
        assert!(spec.is_member(&exact), "{exact} ge {exact}");
    }

    #[test]
    fn check_build_number_exact_spec() {
        let mismatch = 10;
        let exact = 5;
        let spec: BuildNumberSpec = exact.to_string().parse().unwrap();
        assert!(spec.is_member(&exact));
    }
}
