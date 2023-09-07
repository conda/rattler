//! This module contains code to work with build specs in a MatchSpec
// use constraint::OrdConstraint;

mod parse;

use crate::constraint::{operators::OrdOperator, OperatorConstraint, Set};
pub(crate) use parse::ParseBuildNumberSpecError;

/// named type for internal representation for build number
pub type BuildNumber = u64;
/// named type for the Set specified by BuildNumberOperator on BuildNumber
pub type BuildNumberSpec = OperatorConstraint<BuildNumber, OrdOperator>;

/// Same API as other fields in `crate::match_spec::MatchSpec`
/// Internally relies on
impl BuildNumberSpec {
    pub fn matches(&self, num: &BuildNumber) -> bool {
        self.is_member(num)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_build_number_cmp_spec() {
        let above = 10;
        let below = 1;
        let exact = 5;
        let spec: BuildNumberSpec = (">=".to_string() + &exact.to_string()).parse().unwrap();

        assert_eq!(spec, BuildNumberSpec::new(OrdOperator::Ge, exact));

        assert!(!spec.matches(&below), "{below} not ge {exact}");
        assert!(spec.matches(&above), "{above} ge {exact}");
        assert!(spec.matches(&exact), "{exact} ge {exact}");
    }

    #[test]
    fn check_build_number_exact_spec() {
        let mismatch = 10;
        let exact = 5;
        let spec: BuildNumberSpec = exact.to_string().parse().unwrap();
        assert!(spec.matches(&exact));
        assert!(!spec.matches(&mismatch));
    }
}
