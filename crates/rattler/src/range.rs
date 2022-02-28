use smallvec::{smallvec, SmallVec};
use std::cmp::Ordering;
use std::fmt::Debug;
use std::ops::Bound::{self, Excluded, Included, Unbounded};

type Interval<V> = (Bound<V>, Bound<V>);

#[derive(Debug, Clone, Eq, PartialEq)]
struct Range<V> {
    segments: SmallVec<[Interval<V>; 2]>,
}

impl<V: Clone> Range<V> {
    /// Empty set of versions.
    pub fn none() -> Self {
        Self {
            segments: SmallVec::default(),
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
    pub fn negate(&self) -> Self {
        match self.segments.first() {
            // Complement of ∅ is *
            None => Self::any(),

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

            _ => unreachable!(),
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

impl<V: Ord + Clone + Debug> Range<V> {
    /// Compute the union of two sets of versions.
    pub fn union(&self, other: &Self) -> Self {
        self.negate().intersection(&other.negate()).negate()
    }

    /// Compute the intersection of two sets of versions.
    pub fn intersection(&self, other: &Self) -> Self {
        let mut segments: SmallVec<[Interval<V>; 2]> = Default::default();
        let mut left_iter = self.segments.iter();
        let mut right_iter = other.segments.iter();
        let mut left = left_iter.next();
        let mut right = right_iter.next();
        loop {
            match (left, right) {
                // Both sides contain a positive infinite interval
                (Some((left, Unbounded)), Some((right, Unbounded))) => {
                    let start = max_exclusive(left, right);
                    segments.push((start, Unbounded));
                    break;
                }

                // Both sides contain a negative infinite interval
                (Some((Unbounded, left)), Some((Unbounded, right))) => {
                    let end = min_exclusive(left, right);
                    segments.push((Unbounded, end));
                    break;
                }

                // Left contains a positive infinite interval
                (
                    Some((l @ Included(la), Unbounded)),
                    Some((r1 @ Included(ra), r2 @ Included(rb))),
                )
                | (
                    Some((l @ Included(la), Unbounded)),
                    Some((r1 @ Excluded(ra), r2 @ Included(rb))),
                )
                | (
                    Some((l @ Included(la), Unbounded)),
                    Some((r1 @ Included(ra), r2 @ Excluded(rb))),
                )
                | (
                    Some((l @ Included(la), Unbounded)),
                    Some((r1 @ Excluded(ra), r2 @ Excluded(rb))),
                )
                | (
                    Some((l @ Excluded(la), Unbounded)),
                    Some((r1 @ Included(ra), r2 @ Included(rb))),
                )
                | (
                    Some((l @ Excluded(la), Unbounded)),
                    Some((r1 @ Excluded(ra), r2 @ Included(rb))),
                )
                | (
                    Some((l @ Excluded(la), Unbounded)),
                    Some((r1 @ Included(ra), r2 @ Excluded(rb))),
                )
                | (
                    Some((l @ Excluded(la), Unbounded)),
                    Some((r1 @ Excluded(ra), r2 @ Excluded(rb))),
                ) => match rb.cmp(la) {
                    Ordering::Less => right = right_iter.next(),
                    Ordering::Equal => {
                        if matches!((l,r1), (Included(_), Included(_))) {
                            segments.push((l.clone(), r1.clone()));
                        }
                        for r in right_iter.cloned() {
                            segments.push(r)
                        }
                        break;
                    }
                    Ordering::Greater => {
                        let start = max_exclusive(l,r1);
                        segments.push((start, r2.clone()));
                        for r in right_iter.cloned() {
                            segments.push(r)
                        }
                        break;
                    }
                },

                // Left contains positive interval, right contains negative interval
                (
                    Some((l @ Included(la), Unbounded)),
                    Some((Unbounded, r @ Included(ra))),
                )|
                (
                    Some((Unbounded, r @ Included(ra))),
                    Some((l @ Included(la), Unbounded)),
                ) => {
                    todo!()
                }

                // Left or right has ended
                (None, Some(_)) | (Some(_), None) => {
                    break;
                }

                _ => unreachable!("{:?}, {:?}", left, right),
            }
        }

        Self { segments }
    }
}

fn max_exclusive<V: Ord + Clone>(left: &Bound<V>, right: &Bound<V>) -> Bound<V> {
    match (left, right) {
        (Bound::Included(l1), Bound::Included(r1)) => match l1.cmp(r1) {
            Ordering::Less => Included(r1.clone()),
            Ordering::Equal => Included(r1.clone()),
            Ordering::Greater => Included(l1.clone()),
        },
        (Bound::Excluded(l1), Bound::Included(r1)) => match l1.cmp(r1) {
            Ordering::Less => Included(r1.clone()),
            Ordering::Equal => Excluded(r1.clone()),
            Ordering::Greater => Excluded(l1.clone()),
        },
        (Bound::Included(l1), Bound::Excluded(r1)) => match l1.cmp(r1) {
            Ordering::Less => Excluded(r1.clone()),
            Ordering::Equal => Excluded(l1.clone()),
            Ordering::Greater => Included(l1.clone()),
        },
        (Bound::Excluded(l1), Bound::Excluded(r1)) => match l1.cmp(r1) {
            Ordering::Less => Excluded(r1.clone()),
            Ordering::Equal => Excluded(l1.clone()),
            Ordering::Greater => Excluded(l1.clone()),
        },
        _ => unreachable!(),
    }
}

fn min_exclusive<V: Ord + Clone>(left: &Bound<V>, right: &Bound<V>) -> Bound<V> {
    match (left, right) {
        (Bound::Included(l1), Bound::Included(r1)) => match l1.cmp(r1) {
            Ordering::Less => Included(l1.clone()),
            Ordering::Equal => Included(r1.clone()),
            Ordering::Greater => Included(r1.clone()),
        },
        (Bound::Excluded(l1), Bound::Included(r1)) => match l1.cmp(r1) {
            Ordering::Less => Excluded(l1.clone()),
            Ordering::Equal => Excluded(l1.clone()),
            Ordering::Greater => Included(r1.clone()),
        },
        (Bound::Included(l1), Bound::Excluded(r1)) => match l1.cmp(r1) {
            Ordering::Less => Included(l1.clone()),
            Ordering::Equal => Excluded(r1.clone()),
            Ordering::Greater => Excluded(r1.clone()),
        },
        (Bound::Excluded(l1), Bound::Excluded(r1)) => match l1.cmp(r1) {
            Ordering::Less => Excluded(l1.clone()),
            Ordering::Equal => Excluded(r1.clone()),
            Ordering::Greater => Excluded(r1.clone()),
        },
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::Range as R;

    #[test]
    fn negate() {
        assert_eq!(R::less(2).negate(), R::greater_equal(2));
        assert_eq!(R::less_equal(2).negate(), R::greater(2));
        assert_eq!(R::equal(2).negate(), R::not_equal(2));
        assert_eq!(R::greater(2).negate(), R::less_equal(2));
        assert_eq!(R::greater_equal(2).negate(), R::less(2));
        assert_eq!(R::not_equal(2).negate(), R::equal(2));
    }

    // #[test]
    // fn union() {
    //     assert_eq!(R::less(2).union(&R::greater_equal(3)), R::between(1,2).negate())
    // }

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
    }

    #[test]
    fn one_positive_infinite_intersection() {
        assert_eq!(R::greater_equal(2).intersection(&R::between(1,3)), R::between(2,3));
        assert_eq!(R::less_equal(2).intersection(&R::greater_equal(2)), R::equal(2));
    }
}
