//! This module has some constructs for specifying a Spec as a set constructed
//! from a operator and an element from that set

use crate::build_spec::BuildNumber;
use crate::version::Version;
use crate::version_spec::{EqualityOperator, RangeOperator, StrictRangeOperator};
use std::cmp::{PartialEq, PartialOrd};

///
pub type BuildNumberOperator = OrdOperator;
/// named type for the Set specified by BuildNumberOperator on BuildNumber
pub type BuildNumberConstraint = OperatorConstraint<BuildNumber, BuildNumberOperator>;

/// set of operators that act on `Version`
pub enum VersionOperator {
    Ordering(OrdOperator),
    /// Specifies a range of versions using the strict operators
    StrictRange(StrictRangeOperator),
}
pub type VersionConstraint = OperatorConstraint<Version, VersionOperator>;

/// to specify a
pub enum OrdOperator {
    /// Specifies a range
    Range(RangeOperator),
    /// Specifies an exact
    Exact(EqualityOperator),
}

pub trait Operator<T> {
    fn compares(&self, source: &T, target: &T) -> bool;
}

/// expose is_member so that it can be called for VersionSpec and BuildNumberSpec
pub trait Set<T> {
    fn is_member(&self, elem: &T) -> bool;
}

pub struct OperatorConstraint<T, Op> {
    op: Op,
    elem: T,
}

impl<T, Op> Set<T> for OperatorConstraint<T, Op>
where
    Op: Operator<T>,
{
    fn is_member(&self, elem: &T) -> bool {
        self.op.compares(&self.elem, &elem)
    }
}

impl<T, Op> OperatorConstraint<T, Op> {
    fn new(op: Op, elem: T) -> Self {
        OperatorConstraint { elem, op }
    }
}

impl<T> Operator<T> for RangeOperator
where
    T: PartialOrd,
{
    fn compares(&self, source: &T, target: &T) -> bool {
        match self {
            Self::Greater => target > source,
            Self::GreaterEquals => target >= source,
            Self::Less => target < source,
            Self::LessEquals => target <= source,
        }
    }
}

impl<T> Operator<T> for EqualityOperator
where
    T: PartialEq,
{
    fn compares(&self, source: &T, target: &T) -> bool {
        match self {
            Self::Equals => target == source,
            Self::NotEquals => target != source,
        }
    }
}

impl Operator<Version> for StrictRangeOperator {
    fn compares(&self, source: &Version, target: &Version) -> bool {
        match self {
            Self::StartsWith => target.starts_with(&source),
            Self::NotStartsWith => !target.starts_with(&source),
            Self::Compatible => target.compatible_with(&source),
            Self::NotCompatible => !target.compatible_with(&source),
        }
    }
}

impl<T> Operator<T> for OrdOperator
where
    T: PartialOrd + PartialEq,
{
    fn compares(&self, source: &T, target: &T) -> bool {
        match self {
            Self::Range(op) => op.compares(&source, &target),
            Self::Exact(op) => op.compares(&source, &target),
        }
    }
}

impl Operator<Version> for VersionOperator {
    fn compares(&self, source: &Version, target: &Version) -> bool {
        match self {
            Self::Ordering(op) => op.compares(&source, &target),
            Self::StrictRange(op) => op.compares(&source, &target),
        }
    }
}

mod tests {
    use super::*;

    #[test]
    fn zero_depth_operator() {
        type IntConstraint = OperatorConstraint<i32, RangeOperator>;
        let int_constr: IntConstraint = IntConstraint::new(RangeOperator::Less, 10);

        assert!(int_constr.is_member(&9));
        assert!(!int_constr.is_member(&11));
    }

    #[test]
    fn nonzero_depth_operator() {
        use std::str::FromStr;
        let constraint: VersionConstraint = VersionConstraint::new(
            VersionOperator::Ordering(OrdOperator::Range(RangeOperator::Less)),
            Version::from_str("1.2.3").unwrap(),
            // a way to extend enums transparently would make this less verbose
            // as it will absoltely be a pain to unwrap for matches in version_spec.
        );

        assert!(constraint.is_member(&Version::from_str("1.2.2").unwrap()))
    }
}
