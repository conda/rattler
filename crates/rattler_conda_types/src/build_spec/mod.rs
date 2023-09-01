//! This module contains code to work with build specs in a MatchSpec
pub mod constraint;
pub(crate) mod parse;
use constraint::OrdConstraint;
pub use constraint::Set; // expose Set::is_member

pub type BuildNumber = u64;
// pub type BuildNumberSpec = OrdConstraint<BuildNumber>;

// impl BuildNumberSpec {
//     pub fn matches(&self, b: &BuildNumber) -> bool {
//         self.is_member()(b)
//     }
// }

// #[cfg(test)]
// mod tests {
//     use super::constraint::{OrdConstraint, Set};
//     use super::BuildNumberSpec;

//     #[test]
//     fn check_build_number_cmp_spec() {
//         let above = 10;
//         let below = 1;
//         let exact = 5;
//         let spec: BuildNumberSpec = (">=".to_string() + &exact.to_string()).parse().unwrap();

//         assert!(!spec.matches(&below), "{below} not ge {exact}");
//         assert!(spec.matches(&above), "{above} ge {exact}");
//         assert!(spec.matches(&exact), "{exact} ge {exact}");
//     }

//     #[test]
//     fn check_build_number_exact_spec() {
//         let mismatch = 10;
//         let exact = 5;
//         let spec: BuildNumberSpec = exact.to_string().parse().unwrap();
//         assert!(spec.matches(&exact));
//     }
// }
