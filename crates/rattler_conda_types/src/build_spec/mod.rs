//! This module contains code to work with build specs in a MatchSpec
// use constraint::OrdConstraint;

mod parse;
use crate::constraint::{operators::OrdOperator, OperatorConstraint};

type BuildNumber = u64;
/// named type for the Set specified by BuildNumberOperator on BuildNumber
type BuildNumberSpec = OperatorConstraint<BuildNumber, OrdOperator>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint::Set; // expose is_member

    #[test]
    fn check_build_number_cmp_spec() {
        let above = 10;
        let below = 1;
        let exact = 5;
        let spec: BuildNumberSpec = (">=".to_string() + &exact.to_string()).parse().unwrap();

        assert_eq!(spec, BuildNumberSpec::new(OrdOperator::Ge, exact));

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
