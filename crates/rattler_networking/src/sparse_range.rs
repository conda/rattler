use bisection::{bisect_left, bisect_right};
use itertools::Itertools;
use std::{
    fmt::{Debug, Display, Formatter},
    ops::{Range, RangeInclusive},
};

// A data structure that keeps track of a range of values with potential holes in them.
#[derive(Default, Clone, Eq, PartialEq)]
pub struct SparseRange {
    left: Vec<u64>,
    right: Vec<u64>,
}

impl Display for SparseRange {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.covered_ranges()
                .format_with(", ", |elt, f| f(&format_args!(
                    "{}..={}",
                    elt.start(),
                    elt.end()
                )))
        )
    }
}

impl Debug for SparseRange {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}",)
    }
}

impl SparseRange {
    /// Construct a new sparse range
    pub fn new() -> Self {
        Self::default()
    }

    // Construct a new SparseRange from an initial covered range.
    pub fn from_range(range: Range<u64>) -> Self {
        Self {
            left: vec![range.start],
            right: vec![range.end - 1], // -1 because the stored range are inclusive
        }
    }

    /// Returns the covered ranges
    pub fn covered_ranges(&self) -> impl Iterator<Item = RangeInclusive<u64>> + '_ {
        self.left
            .iter()
            .zip(self.right.iter())
            .map(|(&left, &right)| RangeInclusive::new(left, right))
    }

    pub fn is_covered(&self, range: Range<u64>) -> bool {
        let range_start = range.start;
        let range_end = range.end - 1;

        // Compute the indices of the ranges that are covered by the request
        let left_index = bisect_left(&self.right, &range_start);
        let right_index = bisect_right(&self.left, &(range_end + 1));

        // Get all the range bounds that are covered
        let left_slice = &self.left[left_index..right_index];
        let right_slice = &self.right[left_index..right_index];

        // Compute the bounds of covered range taking into account existing covered ranges.
        let start = left_slice
            .first()
            .map(|&left_bound| left_bound.min(range_start))
            .unwrap_or(range_start);

        // Get the ranges that are missing
        let mut bound = start;
        for (&left_bound, &right_bound) in left_slice.iter().zip(right_slice.iter()) {
            if left_bound > bound {
                return false;
            }
            bound = right_bound + 1;
        }

        let end = right_slice
            .last()
            .map(|&right_bound| right_bound.max(range_end))
            .unwrap_or(range_end);

        bound > end
    }

    /// Updates the current range to also cover the specified range.
    pub fn update(&mut self, range: Range<u64>) {
        if let Some((new_range, _)) = self.cover(range) {
            *self = new_range;
        }
    }

    /// Find the ranges that are uncovered for the specified range together with what the
    /// SparseRange would look like if we covered that range.
    pub fn cover(&self, range: Range<u64>) -> Option<(SparseRange, Vec<RangeInclusive<u64>>)> {
        let range_start = range.start;
        let range_end = range.end - 1;

        // Compute the indices of the ranges that are covered by the request
        let left_index = bisect_left(&self.right, &range_start);
        let right_index = bisect_right(&self.left, &(range_end + 1));

        // Get all the range bounds that are covered
        let left_slice = &self.left[left_index..right_index];
        let right_slice = &self.right[left_index..right_index];

        // Compute the bounds of covered range taking into account existing covered ranges.
        let start = left_slice
            .first()
            .map(|&left_bound| left_bound.min(range_start))
            .unwrap_or(range_start);
        let end = right_slice
            .last()
            .map(|&right_bound| right_bound.max(range_end))
            .unwrap_or(range_end);

        // Get the ranges that are missing
        let mut ranges = Vec::new();
        let mut bound = start;
        for (&left_bound, &right_bound) in left_slice.iter().zip(right_slice.iter()) {
            if left_bound > bound {
                ranges.push(bound..=(left_bound - 1));
            }
            bound = right_bound + 1;
        }
        if bound <= end {
            ranges.push(bound..=end)
        }

        if !ranges.is_empty() {
            let mut new_left = self.left.clone();
            new_left.splice(left_index..right_index, [start]);
            let mut new_right = self.right.clone();
            new_right.splice(left_index..right_index, [end]);
            Some((
                Self {
                    left: new_left,
                    right: new_right,
                },
                ranges,
            ))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod test {
    use super::SparseRange;

    #[test]
    fn test_sparse_range() {
        let range = SparseRange::new();
        assert!(range.covered_ranges().next().is_none());
        assert_eq!(
            range.cover(5..10).unwrap().0,
            SparseRange::from_range(5..10)
        );

        let range = SparseRange::from_range(5..10);
        assert_eq!(range.covered_ranges().collect::<Vec<_>>(), vec![5..=9]);
        assert!(range.is_covered(5..10));
        assert!(range.is_covered(6..9));
        assert!(!range.is_covered(5..11));
        assert!(!range.is_covered(3..8));

        assert_eq!(
            range.cover(3..5),
            Some((SparseRange::from_range(3..10), vec![3..=4]))
        );

        let (range, missing) = range.cover(12..15).unwrap();
        assert_eq!(
            range.covered_ranges().collect::<Vec<_>>(),
            vec![5..=9, 12..=14]
        );
        assert_eq!(missing, vec![12..=14]);
        assert!(range.is_covered(5..10));
        assert!(range.is_covered(12..15));
        assert!(!range.is_covered(5..15));
        assert!(!range.is_covered(11..12));

        let (range, missing) = range.cover(8..14).unwrap();
        assert_eq!(range.covered_ranges().collect::<Vec<_>>(), vec![5..=14]);
        assert_eq!(missing, vec![10..=11]);
    }
}
