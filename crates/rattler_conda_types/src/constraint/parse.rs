use super::*;
use nom::{
    branch::alt,
    bytes::complete::{tag, take_while, take_while1},
    character::complete::char,
    combinator::{opt, value},
    error::{ErrorKind, ParseError},
    sequence::{terminated, tuple},
    IResult,
};
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Clone, Error, Eq, PartialEq)]
pub enum ParseOperatorError {
    #[error("found operator {0}")]
    InvalidOperator(String),
    #[error("expected operator")]
    ExpectedOperator,
    #[error("expected EOF")]
    ExpectedEOF,
}

// impl FromStr for Greater {
//     type Err = ParseOperatorError;
//     fn from_str(s: &str) -> Result<Self, Self::Err> {
//         match s {
//             ">" => Ok(Self(true)),
//             "<=" => Ok(Self(false)),
//             _ => Err(ParseOperatorError::InvalidOperator(s.to_string())),
//         }
//     }
// }

// impl FromStr for Less {
//     type Err = ParseOperatorError;
//     fn from_str(s: &str) -> Result<Self, Self::Err> {
//         match s {
//             "<" => Ok(Self(true)),
//             ">=" => Ok(Self(false)),
//             _ => Err(ParseOperatorError::InvalidOperator(s.to_string())),
//         }
//     }
// }

// impl FromStr for Equal {
//     type Err = ParseOperatorError;
//     fn from_str(s: &str) -> Result<Self, Self::Err> {
//         match s {
//             "==" => Ok(Self(true)),
//             "!=" => Ok(Self(false)),
//             _ => Err(ParseOperatorError::InvalidOperator(s.to_string())),
//         }
//     }
// }

// impl FromStr for StartsWith {
//     type Err = ParseOperatorError;
//     fn from_str(s: &str) -> Result<Self, Self::Err> {
//         match s {
//             "=" => Ok(Self(true)),
//             "!=startswith" => Ok(Self(false)),
//             _ => Err(ParseOperatorError::InvalidOperator(s.to_string())),
//         }
//     }
// }

// impl FromStr for CompatibleWith {
//     type Err = ParseOperatorError;
//     fn from_str(s: &str) -> Result<Self, Self::Err> {
//         match s {
//             "~=" => Ok(Self(true)),
//             "!~=" => Ok(Self(false)),
//             _ => Err(ParseOperatorError::InvalidOperator(s.to_string())),
//         }
//     }
// }

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    enum OrdOperator {
        L(Less),
        G(Greater),
        E(Equal),
    }
    impl Operator<i32> for OrdOperator {
        fn compares(&self, source: &i32, target: &i32) -> bool {
            match self {
                Self::L(op @ Less(_)) => op.compares(&source, &target),
                Self::G(op @ Greater(_)) => op.compares(&source, &target),
                Self::E(op @ Equal(_)) => op.compares(&source, &target),
            }
        }
    }

    type IntConstraint = OperatorConstraint<i32, OrdOperator>;

    // #[test]
    // fn parse_into_specific_operators() {
    //     assert_eq!(">".parse::<Greater>().unwrap(), Greater(true));
    //     assert_eq!("<".parse::<Less>().unwrap(), Less(true));
    //     assert_eq!(">=".parse::<Less>().unwrap(), Less(false));
    //     assert_eq!("<=".parse::<Greater>().unwrap(), Greater(false));
    //     assert_eq!("==".parse::<Equal>().unwrap(), Equal(true));
    //     assert_eq!("!=".parse::<Equal>().unwrap(), Equal(false));
    //     assert_eq!("=".parse::<StartsWith>().unwrap(), StartsWith(true));
    //     assert_eq!(
    //         "!=startswith".parse::<StartsWith>().unwrap(),
    //         StartsWith(false)
    //     );
    //     assert_eq!(
    //         "~=".parse::<CompatibleWith>().unwrap(),
    //         CompatibleWith(true)
    //     );
    //     assert_eq!(
    //         "!~=".parse::<CompatibleWith>().unwrap(),
    //         CompatibleWith(false)
    //     );
    // }
}
