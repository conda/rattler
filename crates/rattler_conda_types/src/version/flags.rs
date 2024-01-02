use std::fmt::{Debug, Formatter};

/// Bitmask indicates if the first component stored in a [`super::Version`] refers to an epoch.
const EPOCH_MASK: u8 = 0b1;
const EPOCH_OFFSET: u8 = 0;

/// Bitmask that indicates what the index is of the first segment that belongs to the local version
/// part. E.g. the part after the '+' sign in `1.2.3+4.5.6`.
const LOCAL_VERSION_MASK: u8 = (1 << 7) - 1;
const LOCAL_VERSION_OFFSET: u8 = 1;

/// Encodes several edge cases in a single byte.
///
/// The first bit is used to indicate whether or not there is an explicit epoch present in the
/// version. If the flag is set it means the first entry in the [`Version::components`] array refers
/// to the epoch instead of to the first component of the first segment.
///
/// The remaining bits are used to encode the index of the first segment that belongs to the local
/// version part instead of to the common part. A value of `0` indicates that there is not local
/// version part.
#[derive(Copy, Clone, Eq, PartialEq, Default)]
#[repr(transparent)]
pub struct Flags(pub(super) u8);

impl Debug for Flags {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Flags")
            .field("has_epoch", &self.has_epoch())
            .field("local_segment_index", &self.local_segment_index())
            .finish()
    }
}

impl Flags {
    /// Sets whether or not the version has an explicit epoch.
    #[must_use]
    pub fn with_has_epoch(self, has_epoch: bool) -> Self {
        let flag = self.0 & !(EPOCH_MASK << EPOCH_OFFSET);
        Self(
            flag | if has_epoch {
                EPOCH_MASK << EPOCH_OFFSET
            } else {
                0
            },
        )
    }

    /// Returns true if this instance indicates that an explicit epoch is present.
    pub fn has_epoch(self) -> bool {
        (self.0 >> EPOCH_OFFSET) & EPOCH_MASK != 0
    }

    /// Sets the index where the local segment starts. Returns `None` if the index is too large to
    /// be stored.
    #[must_use]
    pub fn with_local_segment_index(self, index: u8) -> Option<Self> {
        if index > LOCAL_VERSION_MASK {
            None
        } else {
            Some(Self(
                (self.0 & !(LOCAL_VERSION_MASK << LOCAL_VERSION_OFFSET))
                    | (index << LOCAL_VERSION_OFFSET),
            ))
        }
    }

    /// Returns the index of the first segment that belongs to the local part of the version.
    pub fn local_segment_index(self) -> u8 {
        (self.0 >> LOCAL_VERSION_OFFSET) & LOCAL_VERSION_MASK
    }
}

#[cfg(test)]
mod test {
    use crate::version::flags::Flags;

    #[test]
    fn test_epoch() {
        assert!(!Flags::default().has_epoch());
        assert!(Flags::default().with_has_epoch(true).has_epoch());
        assert!(!Flags::default()
            .with_has_epoch(true)
            .with_has_epoch(false)
            .has_epoch(),);
    }

    #[test]
    fn test_local_segment_idx() {
        assert_eq!(Flags::default().local_segment_index(), 0);
        assert_eq!(
            Flags::default()
                .with_local_segment_index(42)
                .unwrap()
                .local_segment_index(),
            42
        );
        assert_eq!(
            Flags::default()
                .with_local_segment_index(127)
                .unwrap()
                .local_segment_index(),
            127
        );
        assert_eq!(Flags::default().with_local_segment_index(128), None);
    }

    #[test]
    fn test_all_elements() {
        let flags = Flags::default()
            .with_has_epoch(true)
            .with_local_segment_index(101)
            .unwrap();

        assert!(flags.has_epoch());
        assert_eq!(flags.local_segment_index(), 101);
    }
}
