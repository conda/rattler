//! This module has some constructs for specifying a Spec as a set constructed
//! from a operator and an element from that set

use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};
pub mod operators;

/// expose is_member so that it can be called for VersionSpec and BuildNumberSpec
pub trait Set<Element> {
    fn is_member(&self, elem: &Element) -> bool;
}

pub trait Operator<Element> {
    fn compares(&self, source: &Element, target: &Element) -> bool;
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct OperatorConstraint<Element, Op> {
    op: Op,
    elem: Element,
}

impl<Element, Op> Set<Element> for OperatorConstraint<Element, Op>
where
    Op: Operator<Element>,
{
    fn is_member(&self, elem: &Element) -> bool {
        self.op.compares(&self.elem, elem)
    }
}

impl<Element, Op> OperatorConstraint<Element, Op> {
    pub fn new(op: Op, elem: Element) -> Self {
        OperatorConstraint { op, elem }
    }
}

impl<Element, Op> Display for OperatorConstraint<Element, Op>
where
    Element: Display,
    Op: Display,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.op, self.elem)
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     enum OrdOperator {
//         L(Less),
//         G(Greater),
//         E(Equal),
//     }
//     impl Operator<i32> for OrdOperator {
//         fn compares(&self, source: &i32, target: &i32) -> bool {
//             match self {
//                 Self::L(op @ Less(_)) => op.compares(&source, &target),
//                 Self::G(op @ Greater(_)) => op.compares(&source, &target),
//                 Self::E(op @ Equal(_)) => op.compares(&source, &target),
//             }
//         }
//     }

//     type IntConstraint = OperatorConstraint<i32, OrdOperator>;

//     #[test]
//     fn test_ord_operator_constraint() {
//         let int_constr: IntConstraint = IntConstraint::new(OrdOperator::L(Less(true)), 10);
//         assert!(!int_constr.is_member(&10));
//         assert!(!int_constr.is_member(&11));

//         let int_constr: IntConstraint = IntConstraint::new(OrdOperator::G(Greater(false)), 10);
//         assert!(int_constr.is_member(&10));
//         assert!(!int_constr.is_member(&11));

//         let int_constr: IntConstraint = IntConstraint::new(OrdOperator::E(Equal(true)), 10);
//         assert!(int_constr.is_member(&10));
//         assert!(!int_constr.is_member(&11));
//     }

//     #[test]
//     fn show_operator_constraint() {
//         assert_eq!(OperatorConstraint::new(Less(true), 9u32).to_string(), "<9");
//         assert_eq!(
//             OperatorConstraint::new(CompatibleWith(true), "alpha").to_string(),
//             "~=alpha"
//         );
//         assert_eq!(
//             OperatorConstraint::new(Greater(false), 9u32).to_string(),
//             "<=9"
//         );
//     }
// }
