//! This module contains code to work with build specs in a MatchSpec
pub mod constraint;
pub(crate) mod parse;
use constraint::OrdConstraint;
pub use constraint::Set; // expose Set::is_member

pub type BuildNumberSpec = OrdConstraint<u32>;

impl BuildNumberSpec {}

#[cfg(test)]
mod tests {
    use super::constraint::{OrdConstraint, Set};
    use super::BuildNumberSpec;
    #[test]
    fn is_member_parse() {
        // inits
        let above = 10;
        let below = 1;
        let exact = 5;
        let s: String = String::from(">=") + &exact.to_string();

        // Construct to internal type and check
        let spec = s.parse::<OrdConstraint<u32>>().unwrap();
        let matcher = spec.is_member();

        assert!(!matcher(&below), "{below} not ge {exact}");
        assert!(matcher(&above), "{above} ge {exact}");
        assert!(matcher(&exact), "{exact} ge {exact}");
    }

    #[test]
    fn check_as_if_build_number() {
        let above = 10;
        let below = 1;
        let exact = 5;
        let matcher = ">5".to_string();
    }
}
