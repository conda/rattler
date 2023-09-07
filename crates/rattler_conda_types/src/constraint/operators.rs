//! This submodule defines some examples of useful operators
use serde::{Deserialize, Serialize};

use super::*;
use std::fmt::{self, Display, Formatter};

/// An operator for types that impl Eq/PartialEq
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum EqOperator {
    Eq,
    Ne,
}

impl Display for EqOperator {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Eq => write!(f, "=="),
            Self::Ne => write!(f, "!="),
        }
    }
}

impl<Element> Operator<Element> for EqOperator
where
    Element: std::cmp::PartialEq,
{
    fn compares(&self, source: &Element, target: &Element) -> bool {
        match self {
            Self::Eq => target.eq(source),
            Self::Ne => target.ne(source),
        }
    }
}

/// An operator for types that impl Ord/PartialOrd, will suffice for BuildNumber
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum OrdOperator {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

impl Display for OrdOperator {
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

impl<Element> Operator<Element> for OrdOperator
where
    Element: std::cmp::PartialOrd,
{
    fn compares(&self, source: &Element, target: &Element) -> bool {
        match self {
            Self::Eq => target.eq(source),
            Self::Ne => target.ne(source),
            Self::Gt => target.gt(source),
            Self::Ge => target.ge(source),
            Self::Lt => target.lt(source),
            Self::Le => target.le(source),
        }
    }
}
