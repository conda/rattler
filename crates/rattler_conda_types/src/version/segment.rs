use std::fmt;
use std::fmt::{Debug, Formatter};

/// Represents information about a segment in a version. E.g. the part between `.`, `-` or `_`.
///
/// [`Segment`] encodes the number of components, the separator that exists before it and whether
/// or not the segment starts with an implicit default component.
#[derive(Copy, Clone, Eq, PartialEq)]
#[repr(transparent)]
pub struct Segment(u16);

/// Bitmask used to encode segment length.
const COMPONENT_COUNT_MASK: u16 = (1 << 13) - 1;
const COMPONENT_COUNT_OFFSET: u16 = 0;

/// Bitmask used to indicate
const SEGMENT_SEPARATOR_MASK: u16 = 0b11;
const SEGMENT_SEPARATOR_OFFSET: u16 = 13;

/// Bitmask of a single bit that indicates whether the segment starts with an implicit 0.
const IMPLICIT_DEFAULT_ZERO_MASK: u16 = 0b1;
const IMPLICIT_DEFAULT_ZERO_OFFSET: u16 = 15;

impl Segment {
    /// Constructs a new `SegmentInfo`. Returns `None` if the number of components exceeds the
    /// maximum number of components.
    pub fn new(component_count: u16) -> Option<Self> {
        // The number of components is too large.
        if component_count > COMPONENT_COUNT_MASK {
            return None;
        }

        Some(Self(
            (component_count & COMPONENT_COUNT_MASK) << COMPONENT_COUNT_OFFSET,
        ))
    }

    pub fn with_component_count(self, len: u16) -> Option<Self> {
        // The number of components is too large.
        if len > COMPONENT_COUNT_MASK {
            return None;
        }

        let component_mask = (len & COMPONENT_COUNT_MASK) << COMPONENT_COUNT_OFFSET;
        Some(Self(
            self.0 & !(COMPONENT_COUNT_MASK << COMPONENT_COUNT_OFFSET) | component_mask,
        ))
    }

    /// Returns the number of components in this segment
    pub fn len(self) -> u16 {
        (self.0 >> COMPONENT_COUNT_OFFSET) & COMPONENT_COUNT_MASK
    }

    /// Sets whether the segment starts with an implicit default `Component`. This is the case when
    /// a segment starts with a literal.
    pub fn with_implicit_default(self, has_implicit_default: bool) -> Self {
        Self(if has_implicit_default {
            self.0 | ((1 & IMPLICIT_DEFAULT_ZERO_MASK) << IMPLICIT_DEFAULT_ZERO_OFFSET)
        } else {
            self.0 & !(IMPLICIT_DEFAULT_ZERO_MASK << IMPLICIT_DEFAULT_ZERO_OFFSET)
        })
    }

    /// Returns true if the segment starts with an implicit default component.
    pub fn has_implicit_default(self) -> bool {
        self.0 >> IMPLICIT_DEFAULT_ZERO_OFFSET == IMPLICIT_DEFAULT_ZERO_MASK
    }

    /// Set the separator that precedes this segment. Either `.`, `-` or `_`. Returns `None` if the
    /// separator is not recognized.
    pub fn with_separator(self, separator: Option<char>) -> Option<Self> {
        let state = self.0 & !(SEGMENT_SEPARATOR_MASK << SEGMENT_SEPARATOR_OFFSET);
        Some(Self(
            state
                | (match separator {
                    Some('-') => 1,
                    Some('_') => 2,
                    Some('.') => 3,
                    None => 0,
                    _ => return None,
                } << SEGMENT_SEPARATOR_OFFSET),
        ))
    }

    /// Removes the separator from this segment
    pub fn without_separator(self) -> Self {
        Self(self.0 & !(SEGMENT_SEPARATOR_MASK << SEGMENT_SEPARATOR_OFFSET))
    }

    /// Returns the separator that precedes this segment or `None` if there is no separator.
    pub fn separator(self) -> Option<char> {
        match (self.0 >> SEGMENT_SEPARATOR_OFFSET) & SEGMENT_SEPARATOR_MASK {
            0 => None,
            1 => Some('-'),
            2 => Some('_'),
            3 => Some('.'),
            _ => unreachable!(),
        }
    }
}

impl Debug for Segment {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("SegmentInfo")
            .field("len", &self.len())
            .field("has_implicit_default", &self.has_implicit_default())
            .field("separator", &self.separator())
            .finish()
    }
}

#[cfg(test)]
mod test {
    use super::Segment;

    #[test]
    fn test_segment_info() {
        assert_eq!(Segment::new(1).unwrap().len(), 1);
        assert_eq!(Segment::new(42).unwrap().len(), 42);
        assert_eq!(Segment::new(8191).unwrap().len(), 8191);
        assert_eq!(Segment::new(8192), None);

        assert_eq!(
            Segment::new(1)
                .unwrap()
                .with_component_count(1337)
                .unwrap()
                .len(),
            1337
        );
        assert_eq!(
            Segment::new(1)
                .unwrap()
                .with_component_count(4096)
                .unwrap()
                .len(),
            4096
        );

        assert!(!Segment::new(4096).unwrap().has_implicit_default());
        assert!(Segment::new(4096)
            .unwrap()
            .with_implicit_default(true)
            .has_implicit_default(),);
        assert!(!Segment::new(4096)
            .unwrap()
            .with_implicit_default(false)
            .has_implicit_default(),);
        assert!(!Segment::new(4096)
            .unwrap()
            .with_implicit_default(true)
            .with_implicit_default(false)
            .has_implicit_default(),);

        assert_eq!(Segment::new(4096).unwrap().separator(), None);
        assert_eq!(
            Segment::new(4096)
                .unwrap()
                .with_separator(Some('-'))
                .unwrap()
                .separator(),
            Some('-')
        );
        assert_eq!(
            Segment::new(4096)
                .unwrap()
                .with_separator(Some('.'))
                .unwrap()
                .separator(),
            Some('.')
        );
        assert_eq!(
            Segment::new(4096)
                .unwrap()
                .with_separator(Some('_'))
                .unwrap()
                .separator(),
            Some('_')
        );
        assert_eq!(
            Segment::new(4096)
                .unwrap()
                .with_separator(Some('_'))
                .unwrap()
                .separator(),
            Some('_')
        );
        assert_eq!(
            Segment::new(4096)
                .unwrap()
                .with_separator(Some('_'))
                .unwrap()
                .with_separator(None)
                .unwrap()
                .separator(),
            None
        );
    }
}
