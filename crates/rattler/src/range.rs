use smallvec::{smallvec, SmallVec};
use std::cmp::Ordering;
use std::fmt::{Debug, Display, Formatter};
use std::ops::Bound::{self, Excluded, Included, Unbounded};

type Interval<V> = (Bound<V>, Bound<V>);

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Range<V> {
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

                //
                // // Left side is any
                // (Some((Unbounded, Unbounded)), Some(right)) => {
                //     segments.push(right.clone());
                //     for segment in right_iter {
                //         segments.push(segment.clone())
                //     }
                //     break;
                // }
                //
                // // Right side is any
                // (Some(left), Some((Unbounded, Unbounded))) => {
                //     segments.push(left.clone());
                //     for segment in left_iter {
                //         segments.push(segment.clone())
                //     }
                //     break;
                // }
                //
                // // Both sides contain a positive unbounded interval
                // (Some((left, Unbounded)), Some((right, Unbounded))) => {
                //     let start = max_exclusive(left, right);
                //     segments.push((start, Unbounded));
                //     break;
                // }
                //
                // // Both sides contain a negative unbounded interval
                // (
                //     Some((
                //         Unbounded,
                //         left_upper_limit @ Included(left_upper_limit_version)
                //         | left_upper_limit @ Excluded(left_upper_limit_version),
                //     )),
                //     Some((
                //         Unbounded,
                //         right_upper_limit @ Included(right_upper_limit_version)
                //         | right_upper_limit @ Excluded(right_upper_limit_version),
                //     )),
                // ) => {
                //     match left_upper_limit_version.cmp(right_upper_limit_version) {
                //         Ordering::Less => {
                //             left = left_iter.next();
                //             segments.push((Unbounded, left_upper_limit.clone()));
                //         }
                //         Ordering::Equal if matches!(left_upper_limit, Excluded(_)) => {
                //             left = left_iter.next();
                //             segments.push((Unbounded, left_upper_limit.clone()));
                //         },
                //         Ordering::Equal if matches!(right_upper_limit, Excluded(_)) => {
                //             right = right_iter.next();
                //             segments.push((Unbounded, right_upper_limit.clone()));
                //         }
                //         Ordering::Equal => {
                //             left = left_iter.next();
                //             right = right_iter.next();
                //             segments.push((Unbounded, right_upper_limit.clone()));
                //         }
                //         Ordering::Greater => {
                //             right = right_iter.next();
                //             segments.push((Unbounded, right_upper_limit.clone()));
                //         }
                //     }
                // }
                //
                // // Left contains a positive unbounded interval, right is bounded
                // (
                //     Some((
                //         left_lower_limit @ Included(la) | left_lower_limit @ Excluded(la),
                //         Unbounded,
                //     )),
                //     Some((
                //         right_lower_limit @ Included(_) | right_lower_limit @ Excluded(_),
                //         right_upper_limit @ Included(rb) | right_upper_limit @ Excluded(rb),
                //     )),
                // ) => match rb.cmp(la) {
                //     Ordering::Less => right = right_iter.next(),
                //     Ordering::Equal => {
                //         if matches!(
                //             (left_lower_limit, right_upper_limit),
                //             (Included(_), Included(_))
                //         ) {
                //             segments.push((left_lower_limit.clone(), right_upper_limit.clone()));
                //         }
                //         for r in right_iter.cloned() {
                //             segments.push(r)
                //         }
                //         break;
                //     }
                //     Ordering::Greater => {
                //         let start = max_exclusive(left_lower_limit, right_lower_limit);
                //         segments.push((start, right_upper_limit.clone()));
                //         for r in right_iter.cloned() {
                //             segments.push(r)
                //         }
                //         break;
                //     }
                // },
                //
                // // Right contains a positive unbounded interval, Left is bounded
                // (
                //     Some((
                //         right_lower_limit @ Included(_) | right_lower_limit @ Excluded(_),
                //         right_upper_limit @ Included(rb) | right_upper_limit @ Excluded(rb),
                //     )),
                //     Some((
                //         left_lower_limit @ Included(la) | left_lower_limit @ Excluded(la),
                //         Unbounded,
                //     )),
                // ) => match rb.cmp(la) {
                //     Ordering::Less => left = left_iter.next(),
                //     Ordering::Equal => {
                //         if matches!(
                //             (left_lower_limit, right_upper_limit),
                //             (Included(_), Included(_))
                //         ) {
                //             segments.push((left_lower_limit.clone(), right_upper_limit.clone()));
                //         }
                //         for r in left_iter.cloned() {
                //             segments.push(r)
                //         }
                //         break;
                //     }
                //     Ordering::Greater => {
                //         let start = max_exclusive(left_lower_limit, right_lower_limit);
                //         segments.push((start, right_upper_limit.clone()));
                //         for r in left_iter.cloned() {
                //             segments.push(r)
                //         }
                //         break;
                //     }
                // },
                //
                // // Left contains a negative unbounded interval, right is bounded
                // (
                //     Some((
                //         Unbounded,
                //         left_upper_limit @ Included(left_upper_limit_version)
                //         | left_upper_limit @ Excluded(left_upper_limit_version),
                //     )),
                //     Some((
                //         right_lower_limit @ Included(right_lower_limit_version)
                //         | right_lower_limit @ Excluded(right_lower_limit_version),
                //         right_upper_limit @ Included(right_upper_limit_version)
                //         | right_upper_limit @ Excluded(right_upper_limit_version),
                //     )),
                // ) => match right_lower_limit_version.cmp(&left_upper_limit_version) {
                //     Ordering::Greater => {
                //         left = left_iter.next();
                //     }
                //     Ordering::Equal
                //         if !matches!(
                //             (left_upper_limit, right_lower_limit),
                //             (Included(_), Included(_))
                //         ) =>
                //     {
                //         left = left_iter.next();
                //     }
                //     _ => {
                //         let start = min_exclusive(right_lower_limit, left_upper_limit);
                //         match right_upper_limit_version.cmp(left_upper_limit_version) {
                //             Ordering::Less => {
                //                 right = right_iter.next();
                //                 segments.push((start, right_upper_limit.clone()));
                //             }
                //             Ordering::Equal
                //                 if !matches!(
                //                     (left_upper_limit, right_upper_limit),
                //                     (Included(_), Included(_))
                //                 ) =>
                //             {
                //                 right = right_iter.next();
                //                 segments.push((start, right_upper_limit.clone()));
                //             }
                //             Ordering::Equal => {
                //                 right = right_iter.next();
                //                 segments.push((start, Excluded(right_upper_limit_version.clone())));
                //             }
                //             Ordering::Greater => {
                //                 left = left_iter.next();
                //                 segments.push((start, left_upper_limit.clone()));
                //             }
                //         };
                //     }
                // },
                //
                // // Right contains a negative unbounded interval, Left is bounded
                // (
                //     Some((
                //         right_lower_limit @ Included(right_lower_limit_version)
                //         | right_lower_limit @ Excluded(right_lower_limit_version),
                //         right_upper_limit @ Included(right_upper_limit_version)
                //         | right_upper_limit @ Excluded(right_upper_limit_version),
                //     )),
                //     Some((
                //         Unbounded,
                //         left_upper_limit @ Included(left_upper_limit_version)
                //         | left_upper_limit @ Excluded(left_upper_limit_version),
                //     )),
                // ) => match right_lower_limit_version.cmp(&left_upper_limit_version) {
                //     Ordering::Greater => {
                //         right = right_iter.next();
                //     }
                //     Ordering::Equal
                //         if !matches!(
                //             (left_upper_limit, right_lower_limit),
                //             (Included(_), Included(_))
                //         ) =>
                //     {
                //         right = right_iter.next();
                //     }
                //     _ => {
                //         let start = min_exclusive(right_lower_limit, left_upper_limit);
                //         match right_upper_limit_version.cmp(left_upper_limit_version) {
                //             Ordering::Less => {
                //                 left = left_iter.next();
                //                 segments.push((start, right_upper_limit.clone()));
                //             }
                //             Ordering::Equal
                //                 if !matches!(
                //                     (left_upper_limit, right_upper_limit),
                //                     (Included(_), Included(_))
                //                 ) =>
                //             {
                //                 left = left_iter.next();
                //                 segments.push((start, right_upper_limit.clone()));
                //             }
                //             Ordering::Equal => {
                //                 left = left_iter.next();
                //                 segments.push((start, Excluded(right_upper_limit_version.clone())));
                //             }
                //             Ordering::Greater => {
                //                 right = right_iter.next();
                //                 segments.push((start, left_upper_limit.clone()));
                //             }
                //         };
                //     }
                // },
                //
                // // Left contains positive interval, right contains negative interval (-∞ < v > upper_bound, lower_bound < v > ∞)
                // (
                //     Some((
                //         lower_limit @ Included(lower_limit_version)
                //         | lower_limit @ Excluded(lower_limit_version),
                //         Unbounded,
                //     )),
                //     Some((
                //         Unbounded,
                //         upper_limit @ Included(upper_limit_version)
                //         | upper_limit @ Excluded(upper_limit_version),
                //     )),
                // )
                // | (
                //     Some((
                //         Unbounded,
                //         upper_limit @ Included(upper_limit_version)
                //         | upper_limit @ Excluded(upper_limit_version),
                //     )),
                //     Some((
                //         lower_limit @ Included(lower_limit_version)
                //         | lower_limit @ Excluded(lower_limit_version),
                //         Unbounded,
                //     )),
                // ) => {
                //     match lower_limit_version.cmp(upper_limit_version) {
                //         Ordering::Less => segments.push((lower_limit.clone(), upper_limit.clone())),
                //         Ordering::Equal => match (lower_limit, upper_limit) {
                //             (Included(lower), Included(upper)) => {
                //                 segments.push((Included(lower.clone()), Included(upper.clone())))
                //             }
                //             _ => {}
                //         },
                //         Ordering::Greater => {}
                //     };
                //     break;
                // }
                //
                // // Left and right are completely bounded
                // (
                //     Some((
                //         left_lower_limit @ Included(left_lower_limit_version)
                //         | left_lower_limit @ Excluded(left_lower_limit_version),
                //         left_upper_limit @ Included(left_upper_limit_version)
                //         | left_upper_limit @ Excluded(left_upper_limit_version),
                //     )),
                //     Some((
                //         right_lower_limit @ Included(right_lower_limit_version)
                //         | right_lower_limit @ Excluded(right_lower_limit_version),
                //         right_upper_limit @ Included(right_upper_limit_version)
                //         | right_upper_limit @ Excluded(right_upper_limit_version),
                //     )),
                // ) => {
                //     // Check if the left range is completely disjoint and in front of the right
                //     let is_left_disjoint =
                //         match left_upper_limit_version.cmp(right_lower_limit_version) {
                //             Ordering::Less => true,
                //             Ordering::Equal
                //                 if !matches!(
                //                     (left_upper_limit, right_lower_limit),
                //                     (Included(_), Included(_))
                //                 ) =>
                //             {
                //                 true
                //             }
                //             _ => false,
                //         };
                //     if is_left_disjoint {
                //         left = left_iter.next();
                //         continue;
                //     }
                //
                //     // Check if the right range is completely disjoint and in front of the left
                //     let is_right_disjoint =
                //         match right_upper_limit_version.cmp(left_lower_limit_version) {
                //             Ordering::Less => true,
                //             Ordering::Equal
                //                 if !matches!(
                //                     (left_lower_limit, right_upper_limit),
                //                     (Included(_), Included(_))
                //                 ) =>
                //             {
                //                 true
                //             }
                //             _ => false,
                //         };
                //     if is_right_disjoint {
                //         right = right_iter.next();
                //         continue;
                //     }
                //
                //     let start = max_exclusive(left_lower_limit, right_lower_limit);
                //     match left_upper_limit_version.cmp(right_upper_limit_version) {
                //         Ordering::Less => {
                //             segments.push((start, left_upper_limit.clone()));
                //             left = left_iter.next();
                //         }
                //         Ordering::Equal => {
                //             let end = match (left_upper_limit, right_upper_limit) {
                //                 (Included(v), Included(_)) => Included(v.clone()),
                //                 (Included(_), Excluded(v)) => Excluded(v.clone()),
                //                 (Excluded(v), Included(_)) => Excluded(v.clone()),
                //                 (Excluded(v), Excluded(_)) => Excluded(v.clone()),
                //                 _ => unreachable!(),
                //             };
                //             segments.push((start, end));
                //             right = right_iter.next();
                //             left = left_iter.next();
                //         }
                //         Ordering::Greater => {
                //             segments.push((start, right_upper_limit.clone()));
                //             right = right_iter.next();
                //         }
                //     }
                // }

                // Left or right has ended
                (None, _) | (_, None) => {
                    break;
                }
            }
        }

        Self { segments }
    }
}

// enum LeftRightEqual<V> {
//     Left(V),
//     Right(V),
//     Equal(Bound<V>),
// }
//
// fn max_exclusive<V: Ord + Clone>(left: &Bound<V>, right: &Bound<V>) -> LeftRightEqual<V> {
//     match (left, right) {
//         (Bound::Unbounded, Bound::Unbounded) => LeftRightEqual::Equal(Unbounded),
//         (Bound::Unbounded, Bound::Excluded(r1)|Bound::Included(r1)) => LeftRightEqual::Right(right.clone()),
//         (Bound::Excluded(r1)|Bound::Included(r1), Bound::Unbounded) => LeftRightEqual::Left(left.clone()),
//         (Bound::Included(l1)|Bound::Excluded(l1), Bound::Included(r1)|Bound::Excluded(r1)) => match l1.cmp(r1) {
//             Ordering::Less => LeftRightEqual::Right(r1),
//             Ordering::Equal => match (left, right) {
//                 (Bound::Included(_), Bound::Excluded(_)) => LeftRightEqual::Equal(right.clone()),
//                 (Bound::Excluded(_), Bound::Excluded(_)) => LeftRightEqual::Equal(right.clone()),
//                 (Bound::Excluded(_), Bound::Included(_)) => LeftRightEqual::Equal(left.clone()),
//                 (Bound::Included(_), Bound::Included(_)) => LeftRightEqual::Equal(left.clone()),
//                 _ => unreachable!()
//             },
//             Ordering::Greater => LeftRightEqual::Left(l1),
//         }
//     }
// }

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
                    (Unbounded, Included(v)) => write!(f, "v <= {v}")?,
                    (Unbounded, Excluded(v)) => write!(f, "v < {v}")?,
                    (Included(v), Unbounded) => write!(f, "{v} <= v")?,
                    (Included(v), Included(b)) => {
                        if v == b {
                            write!(f, "{v}")?
                        } else {
                            write!(f, "{v} <= v <= {b}")?
                        }
                    }
                    (Included(v), Excluded(b)) => write!(f, "{v} <= v < {b}")?,
                    (Excluded(v), Unbounded) => write!(f, "{v} < v")?,
                    (Excluded(v), Included(b)) => write!(f, "{v} < v <= {b}")?,
                    (Excluded(v), Excluded(b)) => write!(f, "{v} < v < {b}")?,
                };
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Range as R;

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
        assert_eq!(format!("{}", R::between(1, 3)), String::from("1 <= v < 3"));
        assert_eq!(format!("{}", R::<i32>::any()), String::from("*"));
        assert_eq!(
            format!("{}", R::between(1, 3).negate()),
            String::from("v < 1, 3 <= v")
        );
    }
}
