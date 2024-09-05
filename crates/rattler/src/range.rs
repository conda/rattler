//! Ranges are constraints defining sets of versions.
//!
//! Concretely, those constraints correspond to any set of versions
//! representable as the concatenation, union, and complement
//! of the ranges building blocks.

use pubgrub::version_set::VersionSet;
use smallvec::{smallvec, SmallVec};
use std::cmp::Ordering;
use std::fmt::{Debug, Display, Formatter};
use std::ops::Bound::{self, Excluded, Included, Unbounded};

type Interval<V> = (Bound<V>, Bound<V>);

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct Range<V> {
    segments: SmallVec<[Interval<V>; 2]>,
}

impl<V: Clone> Range<V> {
    /// Empty set of versions.
    pub fn none() -> Self {
        Self {
            segments: SmallVec::new_const(),
        }
    }

    /// Set of all possible versions
    pub fn any() -> Self {
        Self {
            segments: smallvec![(Unbounded, Unbounded)],
        }
    }

    /// Set containing exactly one version
    pub fn equal(v: V) -> Self {
        Self {
            segments: smallvec![(Included(v.clone()), Included(v))],
        }
    }

    /// Set containing all versions expect one
    pub fn not_equal(v: V) -> Self {
        Self {
            segments: smallvec![(Unbounded, Excluded(v.clone())), (Excluded(v), Unbounded)],
        }
    }

    /// Set of all versions higher or equal to some version
    pub fn greater_equal(v: V) -> Self {
        Self {
            segments: smallvec![(Included(v), Unbounded)],
        }
    }

    /// Set of all versions higher to some version
    pub fn greater(v: V) -> Self {
        Self {
            segments: smallvec![(Excluded(v), Unbounded)],
        }
    }

    /// Set of all versions lower to some version
    pub fn less(v: V) -> Self {
        Self {
            segments: smallvec![(Unbounded, Excluded(v))],
        }
    }

    /// Set of all versions lower or equal to some version
    pub fn less_equal(v: V) -> Self {
        Self {
            segments: smallvec![(Unbounded, Included(v))],
        }
    }

    /// Set of versions greater or equal to `v1` but less than `v2`.
    pub fn between(v1: V, v2: V) -> Self {
        Self {
            segments: smallvec![(Included(v1), Excluded(v2))],
        }
    }
}

impl<V: Clone> Range<V> {
    /// Returns the complement of this Range.
    pub fn negate(&self) -> Self {
        match self.segments.first() {
            // Complement of ∅ is ∞
            None => Self::any(),

            // Complement of ∞ is ∅
            Some((Unbounded, Unbounded)) => Self::none(),

            // First high bound is +∞
            Some((Included(v), Unbounded)) => Self::less(v.clone()),
            Some((Excluded(v), Unbounded)) => Self::less_equal(v.clone()),

            Some((Unbounded, Included(v))) => {
                Self::negate_segments(Excluded(v.clone()), &self.segments[1..])
            }
            Some((Unbounded, Excluded(v))) => {
                Self::negate_segments(Included(v.clone()), &self.segments[1..])
            }
            Some((Included(_), Included(_)))
            | Some((Included(_), Excluded(_)))
            | Some((Excluded(_), Included(_)))
            | Some((Excluded(_), Excluded(_))) => Self::negate_segments(Unbounded, &self.segments),
        }
    }

    /// Helper function performing the negation of intervals in segments.
    fn negate_segments(start: Bound<V>, segments: &[Interval<V>]) -> Self {
        let mut complement_segments: SmallVec<[Interval<V>; 2]> = Default::default();
        let mut start = start;
        for (v1, v2) in segments {
            complement_segments.push((
                start,
                match v1 {
                    Included(v) => Excluded(v.clone()),
                    Excluded(v) => Included(v.clone()),
                    Unbounded => unreachable!(),
                },
            ));
            start = match v2 {
                Included(v) => Excluded(v.clone()),
                Excluded(v) => Included(v.clone()),
                Unbounded => Unbounded,
            }
        }
        if !matches!(start, Unbounded) {
            complement_segments.push((start, Unbounded));
        }

        Self {
            segments: complement_segments,
        }
    }
}

impl<V: Ord> Range<V> {
    /// Returns true if the this Range contains the specified value.
    pub fn contains(&self, v: &V) -> bool {
        for segment in self.segments.iter() {
            if match segment {
                (Unbounded, Unbounded) => true,
                (Unbounded, Included(end)) => v <= end,
                (Unbounded, Excluded(end)) => v < end,
                (Included(start), Unbounded) => v >= start,
                (Included(start), Included(end)) => v >= start && v <= end,
                (Included(start), Excluded(end)) => v >= start && v < end,
                (Excluded(start), Unbounded) => v > start,
                (Excluded(start), Included(end)) => v > start && v <= end,
                (Excluded(start), Excluded(end)) => v > start && v < end,
            } {
                return true;
            }
        }
        false
    }
}

impl<V: Ord + Clone> Range<V> {
    /// Computes the union of two sets of versions.
    pub fn union(&self, other: &Self) -> Self {
        self.negate().intersection(&other.negate()).negate()
    }

    /// Computes the intersection of two sets of versions.
    pub fn intersection(&self, other: &Self) -> Self {
        let mut segments: SmallVec<[Interval<V>; 2]> = Default::default();
        let mut left_iter = self.segments.iter();
        let mut right_iter = other.segments.iter();
        let mut left = left_iter.next();
        let mut right = right_iter.next();
        loop {
            match (left, right) {
                (Some((left_lower, left_upper)), Some((right_lower, right_upper))) => {
                    // Check if the left range completely smaller than the right range.
                    if let (
                        Included(left_upper_version) | Excluded(left_upper_version),
                        Included(right_lower_version) | Excluded(right_lower_version),
                    ) = (left_upper, right_lower)
                    {
                        match left_upper_version.cmp(right_lower_version) {
                            Ordering::Less => {
                                // Left range is disjoint from the right range.
                                left = left_iter.next();
                                continue;
                            }
                            Ordering::Equal => {
                                if !matches!((left_upper, right_lower), (Included(_), Included(_)))
                                {
                                    // Left and right are overlapping exactly, but one of the bounds is exclusive, therefor the ranges are disjoint
                                    left = left_iter.next();
                                    continue;
                                }
                            }
                            Ordering::Greater => {
                                // Left upper bound is greater than right lower bound, so the lower bound is the right lower bound
                            }
                        }
                    }
                    // Check if the right range completely smaller than the left range.
                    if let (
                        Included(left_lower_version) | Excluded(left_lower_version),
                        Included(right_upper_version) | Excluded(right_upper_version),
                    ) = (left_lower, right_upper)
                    {
                        match right_upper_version.cmp(left_lower_version) {
                            Ordering::Less => {
                                // Right range is disjoint from the left range.
                                right = right_iter.next();
                                continue;
                            }
                            Ordering::Equal => {
                                if !matches!((right_upper, left_lower), (Included(_), Included(_)))
                                {
                                    // Left and right are overlapping exactly, but one of the bounds is exclusive, therefor the ranges are disjoint
                                    right = right_iter.next();
                                    continue;
                                }
                            }
                            Ordering::Greater => {
                                // Right upper bound is greater than left lower bound, so the lower bound is the left lower bound
                            }
                        }
                    }

                    // At this point we know there is an overlap between the versions, find the lowest bound
                    let lower = match (left_lower, right_lower) {
                        (Unbounded, Included(_) | Excluded(_)) => right_lower.clone(),
                        (Included(_) | Excluded(_), Unbounded) => left_lower.clone(),
                        (Unbounded, Unbounded) => Unbounded,
                        (Included(l) | Excluded(l), Included(r) | Excluded(r)) => match l.cmp(r) {
                            Ordering::Less => right_lower.clone(),
                            Ordering::Equal => match (left_lower, right_lower) {
                                (Included(_), Excluded(v)) => Excluded(v.clone()),
                                (Excluded(_), Excluded(v)) => Excluded(v.clone()),
                                (Excluded(v), Included(_)) => Excluded(v.clone()),
                                (Included(_), Included(v)) => Included(v.clone()),
                                _ => unreachable!(),
                            },
                            Ordering::Greater => left_lower.clone(),
                        },
                    };

                    // At this point we know there is an overlap between the versions, find the lowest bound
                    let upper = match (left_upper, right_upper) {
                        (Unbounded, Included(_) | Excluded(_)) => {
                            right = right_iter.next();
                            right_upper.clone()
                        }
                        (Included(_) | Excluded(_), Unbounded) => {
                            left = left_iter.next();
                            left_upper.clone()
                        }
                        (Unbounded, Unbounded) => {
                            left = left_iter.next();
                            right = right_iter.next();
                            Unbounded
                        }
                        (Included(l) | Excluded(l), Included(r) | Excluded(r)) => match l.cmp(r) {
                            Ordering::Less => {
                                left = left_iter.next();
                                left_upper.clone()
                            }
                            Ordering::Equal => match (left_upper, right_upper) {
                                (Included(_), Excluded(v)) => {
                                    right = right_iter.next();
                                    Excluded(v.clone())
                                }
                                (Excluded(_), Excluded(v)) => {
                                    left = left_iter.next();
                                    right = right_iter.next();
                                    Excluded(v.clone())
                                }
                                (Excluded(v), Included(_)) => {
                                    left = left_iter.next();
                                    Excluded(v.clone())
                                }
                                (Included(_), Included(v)) => {
                                    left = left_iter.next();
                                    right = right_iter.next();
                                    Included(v.clone())
                                }
                                _ => unreachable!(),
                            },
                            Ordering::Greater => {
                                right = right_iter.next();
                                right_upper.clone()
                            }
                        },
                    };

                    segments.push((lower, upper));
                }

                // Left or right has ended
                (None, _) | (_, None) => {
                    break;
                }
            }
        }

        Self { segments }
    }
}

impl<V: Display + Eq> Display for Range<V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.segments.is_empty() {
            write!(f, "∅")?;
        } else {
            for (idx, segment) in self.segments.iter().enumerate() {
                if idx > 0 {
                    write!(f, ", ")?;
                }
                match segment {
                    (Unbounded, Unbounded) => write!(f, "*")?,
                    (Unbounded, Included(v)) => write!(f, "<={v}")?,
                    (Unbounded, Excluded(v)) => write!(f, "<{v}")?,
                    (Included(v), Unbounded) => write!(f, ">={v}")?,
                    (Included(v), Included(b)) => {
                        if v == b {
                            write!(f, "{v}")?
                        } else {
                            write!(f, ">={v},<={b}")?
                        }
                    }
                    (Included(v), Excluded(b)) => write!(f, ">={v}, <{b}")?,
                    (Excluded(v), Unbounded) => write!(f, ">{v}")?,
                    (Excluded(v), Included(b)) => write!(f, ">{v}, <={b}")?,
                    (Excluded(v), Excluded(b)) => write!(f, ">{v}, <{b}")?,
                };
            }
        }
        Ok(())
    }
}

impl<T: Debug + Display + Clone + Eq + Ord> VersionSet for Range<T> {
    type V = T;

    fn empty() -> Self {
        Range::none()
    }

    fn singleton(v: Self::V) -> Self {
        Range::equal(v)
    }

    fn complement(&self) -> Self {
        Range::negate(self)
    }

    fn intersection(&self, other: &Self) -> Self {
        Range::intersection(self, other)
    }

    fn contains(&self, v: &Self::V) -> bool {
        Range::contains(self, v)
    }

    fn full() -> Self {
        Range::any()
    }

    fn union(&self, other: &Self) -> Self {
        Range::union(self, other)
    }
}

#[cfg(test)]
mod tests {
    use super::Range as R;
    use super::Range;
    use proptest::prelude::*;
    use proptest::test_runner::TestRng;
    use smallvec::smallvec;
    use std::ops::Bound::{self, Excluded, Included, Unbounded};

    pub fn strategy() -> impl Strategy<Value = R<usize>> {
        prop::collection::vec(any::<usize>(), 0..10)
            .prop_map(|mut vec| {
                vec.sort_unstable();
                vec.dedup();
                vec
            })
            .prop_perturb(|vec, mut rng| {
                let mut segments = smallvec![];
                let mut iter = vec.into_iter().peekable();
                if let Some(first) = iter.next() {
                    fn next_bound<I: Iterator<Item = usize>>(
                        iter: &mut I,
                        rng: &mut TestRng,
                    ) -> Bound<usize> {
                        if let Some(next) = iter.next() {
                            if rng.gen_bool(0.5) {
                                Included(next)
                            } else {
                                Excluded(next)
                            }
                        } else {
                            Unbounded
                        }
                    }

                    let start = if rng.gen_bool(0.3) {
                        Unbounded
                    } else {
                        if rng.gen_bool(0.5) {
                            Included(first)
                        } else {
                            Excluded(first)
                        }
                    };

                    let end = next_bound(&mut iter, &mut rng);
                    segments.push((start, end));

                    while iter.peek().is_some() {
                        let start = next_bound(&mut iter, &mut rng);
                        let end = next_bound(&mut iter, &mut rng);
                        segments.push((start, end));
                    }
                }
                return Range { segments };
            })
    }

    fn version_strat() -> impl Strategy<Value = usize> {
        any::<usize>()
    }

    #[test]
    fn negate() {
        assert_eq!(R::<usize>::none().negate(), R::<usize>::any());
        assert_eq!(R::<usize>::any().negate(), R::<usize>::none());
        assert_eq!(R::less(2).negate(), R::greater_equal(2));
        assert_eq!(R::less_equal(2).negate(), R::greater(2));
        assert_eq!(R::equal(2).negate(), R::not_equal(2));
        assert_eq!(R::greater(2).negate(), R::less_equal(2));
        assert_eq!(R::greater_equal(2).negate(), R::less(2));
        assert_eq!(R::not_equal(2).negate(), R::equal(2));
        assert_eq!(
            R::less(1).union(&R::greater_equal(3)).negate(),
            R::between(1, 3)
        );
        assert_eq!(
            R::less(1).union(&R::greater_equal(3)),
            R::between(1, 3).negate()
        );
    }

    #[test]
    fn union() {
        assert_eq!(
            R::less(2).union(&R::greater_equal(3)),
            R::between(2, 3).negate()
        )
    }

    #[test]
    fn positive_infinite_intersection() {
        assert_eq!(R::greater(2).intersection(&R::greater(2)), R::greater(2));
        assert_eq!(R::greater(2).intersection(&R::greater(3)), R::greater(3));
        assert_eq!(R::greater(3).intersection(&R::greater(2)), R::greater(3));

        assert_eq!(
            R::greater_equal(2).intersection(&R::greater(1)),
            R::greater_equal(2)
        );
        assert_eq!(
            R::greater_equal(2).intersection(&R::greater(2)),
            R::greater(2)
        );
        assert_eq!(
            R::greater_equal(2).intersection(&R::greater(3)),
            R::greater(3)
        );

        assert_eq!(
            R::greater(1).intersection(&R::greater_equal(2)),
            R::greater_equal(2)
        );
        assert_eq!(
            R::greater(2).intersection(&R::greater_equal(2)),
            R::greater(2)
        );
        assert_eq!(
            R::greater(3).intersection(&R::greater_equal(2)),
            R::greater(3)
        );
    }

    #[test]
    fn negative_infinite_intersection() {
        assert_eq!(R::less(2).intersection(&R::less(2)), R::less(2));
        assert_eq!(R::less(2).intersection(&R::less(3)), R::less(2));
        assert_eq!(R::less(3).intersection(&R::less(2)), R::less(2));

        assert_eq!(R::less_equal(2).intersection(&R::less(1)), R::less(1));
        assert_eq!(R::less_equal(2).intersection(&R::less(2)), R::less(2));
        assert_eq!(R::less_equal(2).intersection(&R::less(3)), R::less_equal(2));

        assert_eq!(R::less(1).intersection(&R::less_equal(2)), R::less(1));
        assert_eq!(R::less(2).intersection(&R::less_equal(2)), R::less(2));
        assert_eq!(R::less(3).intersection(&R::less_equal(2)), R::less_equal(2));

        assert_eq!(
            R::less(1)
                .union(&R::greater_equal(2))
                .intersection(&R::less(3)),
            R::less(1).union(&R::between(2, 3))
        );
    }

    #[test]
    fn one_positive_infinite_intersection() {
        assert_eq!(
            R::greater_equal(2).intersection(&R::between(1, 3)),
            R::between(2, 3)
        );
    }

    #[test]
    fn one_negative_infinite_intersection() {
        assert_eq!(R::less(2).intersection(&R::between(1, 3)), R::between(1, 2));
    }

    #[test]
    fn overlapping_infinite_range() {
        assert_eq!(
            R::less_equal(2).intersection(&R::greater_equal(2)),
            R::equal(2)
        );
        assert_eq!(R::less(2).intersection(&R::greater_equal(2)), R::none());
        assert_eq!(R::less_equal(2).intersection(&R::greater(2)), R::none());
        assert_eq!(R::less(2).intersection(&R::greater(2)), R::none());
        assert_eq!(
            R::less(3).intersection(&R::greater_equal(2)),
            R::between(2, 3)
        );
    }

    #[test]
    fn overlapping_range() {
        assert_eq!(
            R::between(1, 3).intersection(&R::between(2, 4)),
            R::between(2, 3)
        );
        assert_eq!(
            R::between(2, 4).intersection(&R::between(1, 3)),
            R::between(2, 3)
        );
        assert_eq!(R::between(1, 2).intersection(&R::between(2, 4)), R::none());
        assert_eq!(R::between(1, 2).union(&R::between(2, 3)), R::between(1, 3));
    }

    #[test]
    fn contains() {
        assert!(R::any().contains(&1));
        assert!(!R::none().contains(&1));
    }

    #[test]
    fn format() {
        assert_eq!(format!("{}", R::between(1, 3)), String::from(">=1, <3"));
        assert_eq!(format!("{}", R::<i32>::any()), String::from("*"));
        assert_eq!(
            format!("{}", R::between(1, 3).negate()),
            String::from("<1, >=3")
        );
    }

    proptest! {

        // Testing negate ----------------------------------

        #[test]
        fn negate_is_different(range in strategy()) {
            assert_ne!(range.negate(), range);
        }

        #[test]
        fn double_negate_is_identity(range in strategy()) {
            assert_eq!(range.negate().negate(), range);
        }

        #[test]
        fn negate_contains_opposite(range in strategy(), version in version_strat()) {
            assert_ne!(range.contains(&version), range.negate().contains(&version));
        }

        // Testing intersection ----------------------------

        #[test]
        fn intersection_is_symmetric(r1 in strategy(), r2 in strategy()) {
            assert_eq!(r1.intersection(&r2), r2.intersection(&r1));
        }

        #[test]
        fn intersection_with_any_is_identity(range in strategy()) {
            assert_eq!(Range::any().intersection(&range), range);
        }

        #[test]
        fn intersection_with_none_is_none(range in strategy()) {
            assert_eq!(Range::none().intersection(&range), Range::none());
        }

        #[test]
        fn intersection_is_idempotent(r1 in strategy(), r2 in strategy()) {
            assert_eq!(r1.intersection(&r2).intersection(&r2), r1.intersection(&r2));
        }

        #[test]
        fn intersection_is_associative(r1 in strategy(), r2 in strategy(), r3 in strategy()) {
            assert_eq!(r1.intersection(&r2).intersection(&r3), r1.intersection(&r2.intersection(&r3)));
        }

        #[test]
        fn intersection_of_complements_is_none(range in strategy()) {
            assert_eq!(range.negate().intersection(&range), Range::none());
        }

        #[test]
        fn intersection_contains_both(r1 in strategy(), r2 in strategy(), version in version_strat()) {
            assert_eq!(r1.intersection(&r2).contains(&version), r1.contains(&version) && r2.contains(&version));
        }

        // Testing union -----------------------------------

        #[test]
        fn union_of_complements_is_any(range in strategy()) {
            assert_eq!(range.negate().union(&range), Range::any());
        }

        #[test]
        fn union_contains_either(r1 in strategy(), r2 in strategy(), version in version_strat()) {
            assert_eq!(r1.union(&r2).contains(&version), r1.contains(&version) || r2.contains(&version));
        }

        // Testing contains --------------------------------

        #[test]
        fn always_contains_exact(version in version_strat()) {
            assert!(Range::equal(version).contains(&version));
        }

        #[test]
        fn contains_negation(range in strategy(), version in version_strat()) {
            assert_ne!(range.contains(&version), range.negate().contains(&version));
        }

        #[test]
        fn contains_intersection(range in strategy(), version in version_strat()) {
            assert_eq!(range.contains(&version), range.intersection(&Range::equal(version)) != Range::none());
        }
    }
}
