use std::fmt::{self, Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Greater(pub bool);
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Less(pub bool);
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Equal(pub bool);
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartsWith(pub bool);
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibleWith(pub bool);

pub trait Operator<Element> {
    fn compares(&self, source: &Element, target: &Element) -> bool;
}

impl<Element> Operator<Element> for Greater
where
    Element: PartialOrd,
{
    fn compares(&self, source: &Element, target: &Element) -> bool {
        match self.0 {
            true => target > source,
            false => !(target > source),
        }
    }
}

impl<Element> Operator<Element> for Less
where
    Element: PartialOrd,
{
    fn compares(&self, source: &Element, target: &Element) -> bool {
        match self.0 {
            true => target < source,
            false => !(target < source),
        }
    }
}

impl<Element> Operator<Element> for Equal
where
    Element: PartialEq,
{
    fn compares(&self, source: &Element, target: &Element) -> bool {
        match self.0 {
            true => target == source,
            false => !(target == source),
        }
    }
}

impl Display for Greater {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            true => write!(f, ">"),
            false => write!(f, "<="),
        }
    }
}

impl Display for Less {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            true => write!(f, "<"),
            false => write!(f, ">="),
        }
    }
}

impl Display for Equal {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            true => write!(f, "=="),
            false => write!(f, "!="),
        }
    }
}

impl Display for StartsWith {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            true => write!(f, "="),
            false => write!(f, "!=startswith"),
        }
    }
}

impl Display for CompatibleWith {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            true => write!(f, "~="),
            false => write!(f, "!~="),
        }
    }
}
