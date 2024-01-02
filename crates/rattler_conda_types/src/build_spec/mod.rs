//! This module contains code to work with "build number spec". It represents the build number key of
//! [`crate::MatchSpec`], e.g.: `>=3,<4`.

pub mod parse;

pub use parse::ParseBuildNumberSpecError;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

/// Named type for the build number of a package instead of explicit u64 floating about the project.
pub type BuildNumber = u64;

/// An operator to compare two versions.
#[allow(missing_docs)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum OrdOperator {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
    Ne,
}

/// Define match from some kind of operator and a specific element
///
/// Ideally we could have some kind of type constraint to guarantee that
/// there's function relating the operator and element into a function that returns bool
/// possible TODO: create `Operator<Element>` trait
#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct OperatorConstraint<Operator, Element> {
    op: Operator,
    rhs: Element,
}

impl<Operator, Element> OperatorConstraint<Operator, Element> {
    /// convenience constructor wrapper
    pub fn new(op: Operator, rhs: Element) -> Self {
        Self { op, rhs }
    }
}

/// Define match from `OrdOperator` and `BuildNumber` as Element
pub type BuildNumberSpec = OperatorConstraint<OrdOperator, BuildNumber>;

impl Display for OrdOperator {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gt => write!(f, ">"),
            Self::Ge => write!(f, ">="),
            Self::Lt => write!(f, "<"),
            Self::Le => write!(f, "<="),
            Self::Eq => write!(f, "=="),
            Self::Ne => write!(f, "!="),
        }
    }
}

impl Display for BuildNumberSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.op, self.rhs)
    }
}

impl BuildNumberSpec {
    /// Returns whether the number matches the specification.
    /// Expected use is within [`crate::MatchSpec::matches`]
    pub fn matches(&self, build_num: &BuildNumber) -> bool {
        match self.op {
            OrdOperator::Gt => build_num.gt(&self.rhs),
            OrdOperator::Ge => build_num.ge(&self.rhs),
            OrdOperator::Lt => build_num.lt(&self.rhs),
            OrdOperator::Le => build_num.le(&self.rhs),
            OrdOperator::Eq => build_num.eq(&self.rhs),
            OrdOperator::Ne => build_num.ne(&self.rhs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BuildNumberSpec, OrdOperator};

    #[test]
    fn test_matches() {
        let test_cases = vec![
            (BuildNumberSpec::new(OrdOperator::Gt, 3), 5, true),
            (BuildNumberSpec::new(OrdOperator::Ge, 3), 5, true),
            (BuildNumberSpec::new(OrdOperator::Lt, 3), 5, false),
            (BuildNumberSpec::new(OrdOperator::Le, 3), 7, false),
            (BuildNumberSpec::new(OrdOperator::Eq, 3), 7, false),
            (BuildNumberSpec::new(OrdOperator::Ne, 3), 7, true),
        ];
        for (spec, test_val, is_match) in test_cases {
            assert_eq!(spec.matches(&test_val), is_match);
        }
    }
}
