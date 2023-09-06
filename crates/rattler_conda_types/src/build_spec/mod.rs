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
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

impl Display for BuildNumberOperator {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Eq => write!(f, "=="),
            Self::Ne => write!(f, "!="),
            Self::Gt => write!(f, ">"),
            Self::Ge => write!(f, ">="),
            Self::Lt => write!(f, "<"),
            Self::Le => write!(f, "<="),
        }
    }
}

impl<Element> constraint::Operator<Element> for BuildNumberOperator
where
    Element: std::cmp::PartialOrd,
{
    fn compares(&self, source: &Element, target: &Element) -> bool {
        match self {
            Self::Eq => target.eq(&source),
            Self::Ne => target.ne(&source),
            Self::Gt => target.gt(&source),
            Self::Ge => target.ge(&source),
            Self::Lt => target.lt(&source),
            Self::Le => target.le(&source),
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

        assert_eq!(spec, BuildNumberSpec::new(BuildNumberOperator::Ge, exact));

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
        assert!(!spec.is_member(&mismatch));
    }
}
