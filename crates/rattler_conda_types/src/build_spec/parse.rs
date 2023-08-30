/// This module contains conversions to and from string representations for items defined in other parts of `build_spec` module
/// including two-way string (attempted) conversion and parsing with nom.
/// nom parsing is completely TODO
use super::constraint::{OrdConstraint, UnstrictOrdering};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use thiserror::Error;

impl UnstrictOrdering {
    pub fn is_operator_char(c: &char) -> bool {
        matches!(c, '=' | '>' | '<' | '!')
    }
}

#[derive(Debug, Clone, Error, Eq, PartialEq)]
pub enum ParseUnstrictOrderingError {
    #[error("invalid operator")]
    InvalidOperator,
    #[error("expected version operator")]
    ExpectedOperator,
}

impl Display for UnstrictOrdering {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            UnstrictOrdering::Equal => write!(f, "=="),
            UnstrictOrdering::NotEqual => write!(f, "!="),
            UnstrictOrdering::Greater => write!(f, ">"),
            UnstrictOrdering::GreaterEqual => write!(f, ">="),
            UnstrictOrdering::Less => write!(f, "<"),
            UnstrictOrdering::LessEqual => write!(f, "<="),
        }
    }
}

impl FromStr for UnstrictOrdering {
    type Err = ParseUnstrictOrderingError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "<" => Ok(Self::Less),
            "<=" => Ok(Self::LessEqual),
            "==" => Ok(Self::Equal),
            ">" => Ok(Self::Greater),
            ">=" => Ok(Self::GreaterEqual),
            "!=" => Ok(Self::NotEqual),
            _ => return Err(ParseUnstrictOrderingError::InvalidOperator),
        }
    }
}

#[derive(Debug, Clone, Error, Eq, PartialEq)]
pub enum ParseOrdConstraintError {
    #[error("could not parse as operator")]
    Operator,
    #[error("could not parse as value of set element")]
    Value,
}

impl FromStr for OrdConstraint<u32> {
    type Err = ParseOrdConstraintError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let op: UnstrictOrdering = s
            .chars()
            .take_while(UnstrictOrdering::is_operator_char)
            .collect::<String>()
            .parse()
            .map_err(|_| ParseOrdConstraintError::Operator)?;
        let num: u32 = s
            .chars()
            .skip_while(UnstrictOrdering::is_operator_char)
            .collect::<String>()
            .parse()
            .map_err(|_| ParseOrdConstraintError::Value)?;

        Ok(OrdConstraint::new(op, num))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_operator_parse() {
        assert_eq!(
            UnstrictOrdering::Less,
            UnstrictOrdering::from_str("<").unwrap()
        );
        assert_eq!(
            UnstrictOrdering::Greater,
            UnstrictOrdering::from_str(">").unwrap()
        );
        assert_eq!(
            UnstrictOrdering::Equal,
            UnstrictOrdering::from_str("==").unwrap()
        );
        assert!(UnstrictOrdering::from_str("~=").is_err());
        assert!(UnstrictOrdering::from_str("5").is_err());
    }

    #[test]
    fn ordering_constraint_parse() {
        let exact = 5;
        let s: String = String::from(">=") + &exact.to_string();
        let constraint: OrdConstraint<u32> =
            OrdConstraint::new(UnstrictOrdering::GreaterEqual, exact);

        assert_eq!(constraint, s.parse::<OrdConstraint<u32>>().unwrap());
    }
}
