/// interface to Set which has an elementwise operation
/// note that this Set is a "math" set, and not a "programming" set.
/// method returned is a closure as this enforces the immutability of set once queries are made of it
// I'm not sure if I should implement this differently, is a set anything other than what its elements are and aren't?
pub trait Set {
    type Matchable;
    fn is_member(&self) -> Box<dyn Fn(&Self::Matchable) -> bool + '_>;
}

/// Enum to represent the types of ordering we can have from ordered types.
/// Intended use is with constraints on build numbers which are numeric
/// Potential later use for crate::Version which are ordered but not Eq
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum UnstrictOrdering {
    Less,
    LessEqual,
    Equal,
    Greater,
    GreaterEqual,
    NotEqual,
}

impl std::ops::Not for UnstrictOrdering {
    type Output = Self;
    fn not(self) -> Self::Output {
        match self {
            Self::Equal => Self::NotEqual,
            Self::NotEqual => Self::Equal,
            Self::Greater => Self::GreaterEqual,
            Self::GreaterEqual => Self::Less,
            Self::Less => Self::LessEqual,
            Self::LessEqual => Self::Greater,
        }
    }
}

/// This describes a constraint via a compare clause and an element
/// Note that the compare clause needs to be meaningful for the element
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct OrdConstraint<T> {
    ordering: UnstrictOrdering,
    element: T,
}

impl<T> OrdConstraint<T> {
    pub fn new(ordering: UnstrictOrdering, element: T) -> Self {
        OrdConstraint { ordering, element }
    }
}

/// impl Set for OrdConstraint allows to query if a given element is a member of the set
/// defined by the OrdConstraint
impl<T> Set for OrdConstraint<T>
where
    T: Ord + Copy,
{
    type Matchable = T;
    fn is_member(&self) -> Box<dyn Fn(&Self::Matchable) -> bool + '_> {
        match self.ordering {
            UnstrictOrdering::Less => Box::new(|&other| other < self.element),
            UnstrictOrdering::LessEqual => Box::new(|&other| other <= self.element),
            UnstrictOrdering::Equal => Box::new(|&other| other == self.element),
            UnstrictOrdering::Greater => Box::new(|&other| other > self.element),
            UnstrictOrdering::GreaterEqual => Box::new(|&other| other >= self.element),
            UnstrictOrdering::NotEqual => Box::new(|&other| other != self.element),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_constraint_membership() {
        let above = 10;
        let below = 1;
        let exact = 5;
        let constraint: OrdConstraint<u32> =
            OrdConstraint::new(UnstrictOrdering::GreaterEqual, exact);
        let matcher = constraint.is_member();

        assert!(!matcher(&below), "{below} not ge {exact}");
        assert!(matcher(&above), "{above} ge {exact}");
        assert!(matcher(&exact), "{exact} ge {exact}");
    }
}
