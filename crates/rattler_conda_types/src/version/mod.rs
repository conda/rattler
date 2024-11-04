use std::{
    borrow::Cow,
    cell::RefCell,
    cmp::Ordering,
    collections::Bound,
    fmt,
    fmt::{Debug, Display, Formatter},
    hash::{Hash, Hasher},
    iter,
    ops::RangeBounds,
};

use itertools::{Either, EitherOrBoth, Itertools};
pub use parse::{ParseVersionError, ParseVersionErrorKind};
use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use smallvec::SmallVec;

mod flags;
pub(crate) mod parse;
mod segment;
mod with_source;

pub(crate) mod bump;
pub use bump::{VersionBumpError, VersionBumpType};
use flags::Flags;
use segment::Segment;
use thiserror::Error;
pub use with_source::VersionWithSource;

/// This class implements an order relation between version strings. Version
/// strings can contain the usual alphanumeric characters (A-Za-z0-9), separated
/// into segments by dots and underscores. Empty segments (i.e. two consecutive
/// dots, a leading/trailing underscore) are not permitted. An optional epoch
/// number - an integer followed by '!' - can precede the actual version string
/// (this is useful to indicate a change in the versioning scheme itself).
/// Version comparison is case-insensitive.
///
/// Rattler supports six types of version strings:
///
/// * Release versions contain only integers, e.g. '1.0', '2.3.5'.
/// * Pre-release versions use additional letters such as 'a' or 'rc', for
///   example '1.0a1', '1.2.beta3', '2.3.5rc3'.
/// * Development versions are indicated by the string 'dev', for example
///   '1.0dev42', '2.3.5.dev12'.
/// * Post-release versions are indicated by the string 'post', for example
///   '1.0post1', '2.3.5.post2'.
/// * Tagged versions have a suffix that specifies a particular property of
///   interest, e.g. '1.1.parallel'. Tags can be added  to any of the preceding
///   four types. As far as sorting is concerned, tags are treated like strings
///   in pre-release versions.
/// * An optional local version string separated by '+' can be appended to the
///   main (upstream) version string. It is only considered in comparisons when
///   the main versions are equal, but otherwise handled in exactly the same
///   manner.
///
/// To obtain a predictable version ordering, it is crucial to keep the
/// version number scheme of a given package consistent over time.
///
/// Specifically,
///
/// * version strings should always have the same number of components (except
///   for an optional tag suffix or local version string),
/// * letters/strings indicating non-release versions should always occur at the
///   same position.
///
/// Before comparison, version strings are parsed as follows:
///
/// * They are first split into epoch, version number, and local version number
///   at '!' and '+' respectively. If there is no '!', the epoch is set to 0. If
///   there is no '+', the local version is empty.
/// * The version part is then split into components at '.' and '_'.
/// * Each component is split again into runs of numerals and non-numerals
/// * Subcomponents containing only numerals are converted to integers.
/// * Strings are converted to lower case, with special treatment for 'dev' and
///   'post'.
/// * When a component starts with a letter, the fillvalue 0 is inserted to keep
///   numbers and strings in phase, resulting in '1.1.a1' == 1.1.0a1'.
/// * The same is repeated for the local version part.
///
/// # Examples:
///
/// `1.2g.beta15.rc`  =>  `[[0], [1], [2, 'g'], [0, 'beta', 15], [0, 'rc']]`
/// `1!2.15.1_ALPHA`  =>  `[[1], [2], [15], [1, '_alpha']]`
///
/// The resulting lists are compared lexicographically, where the following
/// rules are applied to each pair of corresponding subcomponents:
///
/// * integers are compared numerically
/// * strings are compared lexicographically, case-insensitive
/// * strings are smaller than integers, except
/// * 'dev' versions are smaller than all corresponding versions of other types
/// * 'post' versions are greater than all corresponding versions of other types
/// * if a subcomponent has no correspondent, the missing correspondent is
///   treated as integer 0 to ensure '1.1' == '1.1.0'.
///
/// The resulting order is:
///
/// ```txt
///
///        0.4
///      < 0.4.0
///      < 0.4.1.rc
///     == 0.4.1.RC   # case-insensitive comparison
///      < 0.4.1
///      < 0.5a1
///      < 0.5b3
///      < 0.5C1      # case-insensitive comparison
///      < 0.5
///      < 0.9.6
///      < 0.960923
///      < 1.0
///      < 1.1dev1    # special case 'dev'
///      < 1.1_       # appended underscore is special case for openssl-like versions
///      < 1.1a1
///      < 1.1.0dev1  # special case 'dev'
///     == 1.1.dev1   # 0 is inserted before string
///      < 1.1.a1
///      < 1.1.0rc1
///      < 1.1.0
///     == 1.1
///      < 1.1.0post1 # special case 'post'
///     == 1.1.post1  # 0 is inserted before string
///      < 1.1post1   # special case 'post'
///      < 1996.07.12
///      < 1!0.4.1    # epoch increased
///      < 1!3.1.1.6
///      < 2!0.4.1    # epoch increased again
/// ```
///
/// Some packages (most notably openssl) have incompatible version conventions.
/// In particular, openssl interprets letters as version counters rather than
/// pre-release identifiers. For openssl, the relation
///
/// 1.0.1 < 1.0.1a  =>  False  # should be true for openssl
///
/// holds, whereas conda packages use the opposite ordering. You can work-around
/// this problem by appending an underscore to plain version numbers:
///
/// 1.0.1_ < 1.0.1a =>  True   # ensure correct ordering for openssl
#[derive(Clone, Eq)]
pub struct Version {
    /// Individual components of the version.
    ///
    /// We store a maximum of 3 components on the stack. If a version consists
    /// of more components they are stored on the heap instead. We choose 3
    /// here because most versions only consist of 3 components.
    ///
    /// So for the version `1.2g.beta15.rc` this stores:
    ///
    /// [1, 2, 'g', 0, 'beta', 15, 0, 'rc']
    components: ComponentVec,

    /// Information on each individual segment. Segments group different
    /// components together.
    ///
    /// So for the version `1.2g.beta15.rc` this stores:
    ///
    /// [1,2,3,2]
    ///
    /// e.g. `1` consists of 1 component
    ///      `2g` consists of 2 components (`2` and `g`)
    ///      `beta15` consists of 3 components (`0`, `beta` and `15`). Segments
    /// must always start             with a number.
    ///      `rc` consists of 2 components (`0`, `rc`). Segments must always
    /// start with a number.
    segments: SegmentVec,

    /// Flags to indicate edge cases
    /// The first bit indicates whether or not this version has an epoch.
    /// The rest of the bits indicate from which segment the local version
    /// starts or 0 if there is no local version.
    flags: Flags,
}

type ComponentVec = SmallVec<[Component; 3]>;
type SegmentVec = SmallVec<[Segment; 4]>;

/// Error that can occur when extending a version to a certain length.
#[derive(Error, Debug, PartialEq)]

pub enum VersionExtendError {
    /// The version is too long (there is a maximum number of segments allowed)
    #[error("the version is too long")]
    VersionTooLong,
}

impl Version {
    /// Constructs a version with just a major component and no other
    /// components, e.g. "1".
    pub fn major(major: u64) -> Version {
        Version {
            components: smallvec::smallvec![Component::Numeral(major)],
            segments: smallvec::smallvec![Segment::new(1).unwrap()],
            flags: Flags(0),
        }
    }

    /// Returns true if this version has an epoch.
    pub fn has_epoch(&self) -> bool {
        self.flags.has_epoch()
    }

    /// Returns true if this version has a local version defined
    pub fn has_local(&self) -> bool {
        self.flags.local_segment_index() > 0
    }

    /// Returns the index of the first segment that belongs to the local version
    /// or `None` if there is no local version
    fn local_segment_index(&self) -> Option<usize> {
        let index = self.flags.local_segment_index();
        if index > 0 {
            Some(index as usize)
        } else {
            None
        }
    }

    /// Returns the epoch part of the version. If the version did not specify an
    /// epoch `0` is returned.
    pub fn epoch(&self) -> u64 {
        self.epoch_opt().unwrap_or(0)
    }

    /// Returns the epoch part of the version or `None` if the version did not
    /// specify an epoch.
    pub fn epoch_opt(&self) -> Option<u64> {
        if self.has_epoch() {
            Some(
                self.components[0]
                    .as_number()
                    .expect("if there is an epoch it must be the first component"),
            )
        } else {
            None
        }
    }

    /// Returns the individual segments of the version.
    pub fn segments(
        &self,
    ) -> impl DoubleEndedIterator<Item = SegmentIter<'_>> + ExactSizeIterator + '_ {
        let mut idx = usize::from(self.has_epoch());
        let version_segments = if let Some(local_index) = self.local_segment_index() {
            &self.segments[..local_index]
        } else {
            &self.segments[..]
        };
        version_segments.iter().map(move |&segment| {
            let start = idx;
            idx += segment.len() as usize;
            SegmentIter {
                offset: start,
                version: self,
                segment,
            }
        })
    }

    /// Returns the segments that belong the local part of the version.
    ///
    /// The local part of a a version is the part behind the (optional) `+`.
    /// E.g.:
    ///
    /// ```text
    /// 1.2+3.2.1-alpha0
    ///     ^^^^^^^^^^^^ This is the local part of the version
    /// ```
    pub fn local_segments(
        &self,
    ) -> impl DoubleEndedIterator<Item = SegmentIter<'_>> + ExactSizeIterator + '_ {
        if let Some(start) = self.local_segment_index() {
            let mut idx = usize::from(self.has_epoch());
            idx += self.segments[..start]
                .iter()
                .map(|segment| segment.len() as usize)
                .sum::<usize>();
            let version_segments = &self.segments[start..];
            Either::Left(version_segments.iter().map(move |&segment| {
                let start = idx;
                idx += segment.len() as usize;
                SegmentIter {
                    offset: start,
                    version: self,
                    segment,
                }
            }))
        } else {
            Either::Right(iter::empty())
        }
    }

    /// Tries to extract the major and minor versions from the version. Returns
    /// None if this instance doesnt appear to contain a major and minor
    /// version.
    pub fn as_major_minor(&self) -> Option<(u64, u64)> {
        let mut segments = self.segments();
        let major_segment = segments.next()?;
        let minor_segment = segments.next()?;

        if major_segment.component_count() == 1 && minor_segment.component_count() == 1 {
            Some((
                major_segment
                    .components()
                    .next()
                    .and_then(Component::as_number)?,
                minor_segment
                    .components()
                    .next()
                    .and_then(Component::as_number)?,
            ))
        } else {
            None
        }
    }

    /// Returns true if this is considered a dev version.
    ///
    /// If a version has a single component named "dev" it is considered to be a
    /// dev version.
    pub fn is_dev(&self) -> bool {
        self.segments()
            .flat_map(|segment| segment.components())
            .any(Component::is_dev)
    }

    /// Check if this version version and local strings start with the same as
    /// other.
    pub fn starts_with(&self, other: &Self) -> bool {
        self.epoch() == other.epoch()
            && segments_starts_with(self.segments(), other.segments())
            && segments_starts_with(self.local_segments(), other.local_segments())
    }

    /// Returns true if this version is compatible with the given `other`.
    pub fn compatible_with(&self, other: &Self) -> bool {
        self.ge(other)
            && self.epoch() == other.epoch()
            // Remove the last segment from the limit.
            && segments_starts_with(self.segments(), other.segments().rev().skip(1).rev())
            // Local version comparison remains the same
            && segments_starts_with(self.local_segments(), other.local_segments())
    }

    /// Returns a new version with only the given segments.
    ///
    /// Calling this function on a version that looks like `1.3a.4-alpha3` with
    /// the range `[1..3]` will return the version: `3a.4`.
    pub fn with_segments(&self, segments: impl RangeBounds<usize>) -> Option<Version> {
        // Determine the actual bounds to use
        let segment_count = self.segment_count();
        let start_segment_idx = match segments.start_bound() {
            Bound::Included(idx) => *idx,
            Bound::Excluded(idx) => *idx + 1,
            Bound::Unbounded => 0,
        };
        let end_segment_idx = match segments.end_bound() {
            Bound::Included(idx) => *idx + 1,
            Bound::Excluded(idx) => *idx,
            Bound::Unbounded => segment_count,
        };
        if start_segment_idx >= segment_count
            || end_segment_idx > segment_count
            || start_segment_idx >= end_segment_idx
        {
            return None;
        }

        let mut components = SmallVec::<[Component; 3]>::default();
        let mut segments = SmallVec::<[Segment; 4]>::default();
        let mut flags = Flags::default();

        // Copy the epoch
        if self.has_epoch() {
            components.push(self.epoch().into());
            flags = flags.with_has_epoch(true);
        }

        // Copy the segments and components of the common version
        for (segment_idx, segment_iter) in self
            .segments()
            .skip(start_segment_idx)
            .take(end_segment_idx - start_segment_idx)
            .enumerate()
        {
            let segment = if segment_idx == 0 {
                segment_iter.segment.without_separator()
            } else {
                segment_iter.segment
            };
            segments.push(segment);

            // We skip over implicit default `0` components because we also copy
            // the implicit default flag so it would result in double-`0`s.
            let implicit_default = usize::from(segment_iter.has_implicit_default());
            for component in segment_iter.components().skip(implicit_default) {
                components.push(component.clone());
            }
        }

        // Copy the segments and components of the local version
        let local_start_idx = segments.len();
        for segment_iter in self.local_segments() {
            segments.push(segment_iter.segment);

            let implicit_default = usize::from(segment_iter.has_implicit_default());
            for component in segment_iter.components().skip(implicit_default) {
                components.push(component.clone());
            }
        }

        if self.has_local() {
            flags = u8::try_from(local_start_idx)
                .ok()
                .and_then(|idx| flags.with_local_segment_index(idx))
                .expect("the number of segments must always be smaller so this should never fail");
        }

        Some(Version {
            components,
            segments,
            flags,
        })
    }

    /// Pops the specified number of segments from the version. Returns `None`
    /// if the resulting version would become invalid because it no longer
    /// contains any segments.
    pub fn pop_segments(&self, n: usize) -> Option<Version> {
        let segment_count = self.segment_count();
        if segment_count < n {
            None
        } else {
            self.with_segments(..segment_count - n)
        }
    }

    /// Returns the number of segments in the version. Segments are the part of
    /// the version separated by dots or dashes.
    pub fn segment_count(&self) -> usize {
        if let Some(local_index) = self.local_segment_index() {
            local_index
        } else {
            self.segments.len()
        }
    }

    /// Returns either this [`Version`] or a new [`Version`] where the local
    /// version part has been removed.
    pub fn strip_local(&self) -> Cow<'_, Version> {
        if self.has_local() {
            let mut components = SmallVec::<[Component; 3]>::default();
            let mut segments = SmallVec::<[Segment; 4]>::default();
            let mut flags = Flags::default();

            // Add the epoch
            if let Some(epoch) = self.epoch_opt() {
                components.push(epoch.into());
                flags = flags.with_has_epoch(true);
            }

            // Copy the segments
            for segment_iter in self.segments() {
                segments.push(segment_iter.segment);
                for component in segment_iter.components() {
                    components.push(component.clone());
                }
            }

            Cow::Owned(Version {
                components,
                segments,
                flags,
            })
        } else {
            Cow::Borrowed(self)
        }
    }

    /// Extend the version to the specified length by adding default components
    /// (0s). If the version is already longer than the specified length it
    /// is returned as is.
    pub fn extend_to_length(&self, length: usize) -> Result<Cow<'_, Version>, VersionExtendError> {
        if self.segment_count() >= length {
            return Ok(Cow::Borrowed(self));
        }

        // copy everything up to local version
        let mut segments = self.segments[..self.segment_count()].to_vec();
        let components_end = segments.iter().map(|s| s.len() as usize).sum::<usize>()
            + usize::from(self.has_epoch());
        let mut components = self.components.clone()[..components_end].to_vec();

        // unwrap is OK here because these should be fine to construct
        let segment = Segment::new(1).unwrap().with_separator(Some('.')).unwrap();

        for _ in 0..(length - self.segment_count()) {
            components.push(Component::Numeral(0));
            segments.push(segment);
        }

        // add local version if it exists
        let flags = if self.has_local() {
            let flags = self
                .flags
                .with_local_segment_index(segments.len() as u8)
                .ok_or(VersionExtendError::VersionTooLong)?;
            for segment_iter in self.local_segments() {
                for component in segment_iter.components().cloned() {
                    components.push(component);
                }
                segments.push(segment_iter.segment);
            }
            flags
        } else {
            self.flags
        };

        Ok(Cow::Owned(Version {
            components: components.into(),
            segments: segments.into(),
            flags,
        }))
    }
}

/// Returns true if the specified segments are considered to start with the
/// other segments.
fn segments_starts_with<
    'a,
    'b,
    A: Iterator<Item = SegmentIter<'a>> + 'a,
    B: Iterator<Item = SegmentIter<'b>> + 'b,
>(
    a: A,
    b: B,
) -> bool {
    for ranges in a.zip_longest(b) {
        let (left, right) = match ranges {
            EitherOrBoth::Both(left, right) => (left, right),
            EitherOrBoth::Left(_) => return true,
            EitherOrBoth::Right(segment) => {
                // If the segment is zero we can skip it. As long as there are
                // only zeros, the version is still considered to start with
                // the other version.
                if segment.is_zero() {
                    continue;
                }
                return false;
            }
        };
        for values in left.components().zip_longest(right.components()) {
            if !match values {
                EitherOrBoth::Both(a, b) => a == b,
                EitherOrBoth::Left(_) => return true,
                EitherOrBoth::Right(_) => return false,
            } {
                return false;
            }
        }
    }
    true
}

impl PartialEq<Self> for Version {
    fn eq(&self, other: &Self) -> bool {
        fn segments_equal<'i, I: Iterator<Item = SegmentIter<'i>>>(a: I, b: I) -> bool {
            for ranges in a.zip_longest(b) {
                let (a_range, b_range) = ranges.map_any(Some, Some).or_default();
                let default = Component::default();
                for components in a_range
                    .iter()
                    .flat_map(SegmentIter::components)
                    .zip_longest(b_range.iter().flat_map(SegmentIter::components))
                {
                    let (a_component, b_component) = match components {
                        EitherOrBoth::Left(l) => (l, &default),
                        EitherOrBoth::Right(r) => (&default, r),
                        EitherOrBoth::Both(l, r) => (l, r),
                    };
                    if a_component != b_component {
                        return false;
                    }
                }
            }
            true
        }

        self.epoch() == other.epoch()
            && segments_equal(self.segments(), other.segments())
            && segments_equal(self.local_segments(), other.local_segments())
    }
}

impl Hash for Version {
    fn hash<H: Hasher>(&self, state: &mut H) {
        fn hash_segments<'i, I: Iterator<Item = SegmentIter<'i>>, H: Hasher>(
            state: &mut H,
            segments: I,
        ) {
            let default = Component::default();
            for segment in segments {
                // The versions `1.0` and `1` are considered equal because a version has an
                // infinite number of default components in each segment. The
                // get an equivalent hash we skip trailing default components
                // when computing the hash
                segment
                    .components()
                    .rev()
                    .skip_while(|c| **c == default)
                    .for_each(|c| c.hash(state));
            }
        }

        self.epoch().hash(state);
        hash_segments(state, self.segments());
        hash_segments(state, self.local_segments());
    }
}

impl Debug for Version {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Version")
            .field(
                "version",
                &SegmentFormatter::new(Some(self.epoch_opt().unwrap_or(0)), self.segments()),
            )
            .field("local", &SegmentFormatter::new(None, self.local_segments()))
            .finish()
    }
}

/// A helper struct to format an iterator of [`SegmentIter`]. Implements both
/// [`std::fmt::Debug`] where segments are displayed as an array of arrays (e.g.
/// `[[1], [2,3,4]]`) and [`std::fmt::Display`] where segments are display in
/// their canonical form (e.g. `1.2-rc2`).
struct SegmentFormatter<'v, I: Iterator<Item = SegmentIter<'v>> + 'v> {
    inner: RefCell<Option<(Option<u64>, I)>>,
}

impl<'v, I: Iterator<Item = SegmentIter<'v>> + 'v> SegmentFormatter<'v, I> {
    pub fn new(epoch: Option<u64>, iter: I) -> Self {
        Self {
            inner: RefCell::new(Some((epoch, iter))),
        }
    }
}

impl<'v, I: Iterator<Item = SegmentIter<'v>> + 'v> fmt::Debug for SegmentFormatter<'v, I> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let (epoch, iter) = match self.inner.borrow_mut().take() {
            Some(iter) => iter,
            None => panic!("was already formatted once"),
        };

        write!(f, "[")?;
        if let Some(epoch) = epoch {
            write!(f, "[{epoch}], ")?;
        }
        for (idx, segment) in iter.enumerate() {
            if idx > 0 {
                write!(f, ", ")?;
            }
            write!(f, "[{:?}]", segment.components().format(", "))?;
        }
        write!(f, "]")?;

        Ok(())
    }
}

impl<'v, I: Iterator<Item = SegmentIter<'v>> + 'v> fmt::Display for SegmentFormatter<'v, I> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let (epoch, iter) = match self.inner.borrow_mut().take() {
            Some(iter) => iter,
            None => panic!("was already formatted once"),
        };

        if let Some(epoch) = epoch {
            write!(f, "{epoch}!")?;
        }

        for segment in iter {
            if let Some(separator) = segment.separator() {
                write!(f, "{separator}")?;
            }
            let mut components = segment.components();
            if segment.has_implicit_default() {
                let _ = components.next();
            }
            for component in components {
                write!(f, "{component}")?;
            }
        }
        Ok(())
    }
}

/// Either a number, literal or the infinity.
#[derive(Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum Component {
    /// Numeral Component.
    Numeral(u64),

    /// Post should always be ordered greater than anything else.
    Post,

    /// Dev should always be ordered less than anything else.
    Dev,

    /// A generic string identifier. Identifiers are compared lexicographically.
    /// They are always ordered less than numbers.
    Iden(Box<str>),

    /// An underscore or dash.
    UnderscoreOrDash {
        /// Dash flag.
        is_dash: bool,
    },
}

impl Component {
    /// Returns a component as numeric value.
    pub fn as_number(&self) -> Option<u64> {
        match self {
            Component::Numeral(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns a component as mutable numeric value.
    pub fn as_number_mut(&mut self) -> Option<&mut u64> {
        match self {
            Component::Numeral(value) => Some(value),
            _ => None,
        }
    }

    /// Returns a component as iden value
    pub fn as_iden(&self) -> Option<&str> {
        match self {
            Component::Iden(value) => Some(value),
            _ => None,
        }
    }

    /// Returns a component as mutable iden value
    pub fn as_iden_mut(&mut self) -> Option<&mut Box<str>> {
        match self {
            Component::Iden(value) => Some(value),
            _ => None,
        }
    }

    /// Returns a component as string value.
    #[allow(dead_code)]
    pub fn as_string(&self) -> Option<&str> {
        match self {
            Component::Iden(value) => Some(value.as_ref()),
            _ => None,
        }
    }

    /// Checks whether a component is [`Component::Post`]
    #[allow(dead_code)]
    pub fn is_post(&self) -> bool {
        matches!(self, Component::Post)
    }

    /// Checks whether a component is [`Component::Dev`]
    #[allow(dead_code)]
    pub fn is_dev(&self) -> bool {
        matches!(self, Component::Dev)
    }

    /// Checks whether a component is [`Component::Numeral`]
    pub fn is_numeric(&self) -> bool {
        matches!(self, Component::Numeral(_))
    }

    /// Checks whether the component is a zero.
    pub fn is_zero(&self) -> bool {
        matches!(self, Component::Numeral(0))
    }
}

impl From<u64> for Component {
    fn from(num: u64) -> Self {
        Component::Numeral(num)
    }
}

impl From<String> for Component {
    fn from(other: String) -> Self {
        Component::Iden(other.into_boxed_str())
    }
}

impl Default for Component {
    fn default() -> Self {
        Component::Numeral(0)
    }
}

impl Ord for Component {
    #[allow(clippy::match_same_arms)]
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            // Numbers are always ordered higher than strings
            (Component::Numeral(_), Component::Iden(_) | Component::UnderscoreOrDash { .. }) => {
                Ordering::Greater
            }
            (Component::Iden(_) | Component::UnderscoreOrDash { .. }, Component::Numeral(_)) => {
                Ordering::Less
            }

            // Compare numbers and identifiers normally amongst themselves.
            (Component::Numeral(a), Component::Numeral(b)) => a.cmp(b),
            (Component::Iden(a), Component::Iden(b)) => a.cmp(b),
            (Component::Post, Component::Post) => Ordering::Equal,
            (Component::Dev, Component::Dev) => Ordering::Equal,
            (Component::UnderscoreOrDash { .. }, Component::UnderscoreOrDash { .. }) => {
                Ordering::Equal
            }

            // Underscores are sorted before identifiers
            (Component::UnderscoreOrDash { .. }, Component::Iden(_)) => Ordering::Less,
            (Component::Iden(_), Component::UnderscoreOrDash { .. }) => Ordering::Greater,

            // Post is always compared greater than anything else.
            (Component::Post, _) => Ordering::Greater,
            (_, Component::Post) => Ordering::Less,

            // Dev is always compared less than anything else.
            (Component::Dev, _) => Ordering::Less,
            (_, Component::Dev) => Ordering::Greater,
        }
    }
}

impl PartialOrd for Component {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Display for Component {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Component::Numeral(n) => write!(f, "{n}"),
            Component::Iden(s) => write!(f, "{s}"),
            Component::Post => write!(f, "post"),
            Component::Dev => write!(f, "dev"),
            Component::UnderscoreOrDash { is_dash: true } => write!(f, "-"),
            Component::UnderscoreOrDash { is_dash: false } => write!(f, "_"),
        }
    }
}

impl Debug for Component {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Component::Numeral(n) => write!(f, "{n}"),
            Component::Iden(s) => write!(f, "'{s}'"),
            Component::Post => write!(f, "inf"),
            Component::Dev => write!(f, "'DEV'"),
            Component::UnderscoreOrDash { .. } => write!(f, "'_'"),
        }
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        fn cmp_segments<'i, I: Iterator<Item = SegmentIter<'i>>>(a: I, b: I) -> Ordering {
            for ranges in a.zip_longest(b) {
                let (a_range, b_range) = ranges.map_any(Some, Some).or_default();
                for components in a_range
                    .iter()
                    .flat_map(SegmentIter::components)
                    .zip_longest(b_range.iter().flat_map(SegmentIter::components))
                {
                    let default = Component::default();
                    let (a_component, b_component) = match components {
                        EitherOrBoth::Left(l) => (l, &default),
                        EitherOrBoth::Right(r) => (&default, r),
                        EitherOrBoth::Both(l, r) => (l, r),
                    };
                    match a_component.cmp(b_component) {
                        Ordering::Less => return Ordering::Less,
                        Ordering::Equal => {}
                        Ordering::Greater => return Ordering::Greater,
                    }
                }
            }
            Ordering::Equal
        }

        self.epoch()
            .cmp(&other.epoch())
            .then_with(|| cmp_segments(self.segments(), other.segments()))
            .then_with(|| cmp_segments(self.local_segments(), other.local_segments()))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Display for Version {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            SegmentFormatter::new(self.epoch_opt(), self.segments())
        )?;
        if self.has_local() {
            write!(f, "+{}", SegmentFormatter::new(None, self.local_segments()))?;
        }

        Ok(())
    }
}

impl Serialize for Version {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Cow::<'de, str>::deserialize(deserializer)?
            .parse()
            .map_err(D::Error::custom)
    }
}

pub struct SegmentIter<'v> {
    /// Information about the segment we are iterating.
    segment: Segment,

    /// Offset in the components of the version.
    offset: usize,

    /// The version to which the segment belongs
    version: &'v Version,
}

impl<'v> SegmentIter<'v> {
    /// Returns true if the
    pub fn is_zero(&self) -> bool {
        self.components().all(Component::is_zero)
    }

    /// Returns true if the first component is an implicit default added while
    /// parsing the version. E.g. `2.a` is represented as `2.0a`. The `0` is
    /// added implicitly.
    pub fn has_implicit_default(&self) -> bool {
        self.segment.has_implicit_default()
    }

    /// Returns the separator that is found in from of this segment or `None` if
    /// this segment was not preceded by a separator.
    pub fn separator(&self) -> Option<char> {
        self.segment.separator()
    }

    /// Returns the number of components stored in the version. Note that the
    /// number of components returned by [`Self::components`] might differ
    /// because it might include an implicit default.
    pub fn component_count(&self) -> usize {
        self.segment.len() as usize
    }

    /// Returns an iterator over the components of this segment.
    pub fn components(&self) -> impl DoubleEndedIterator<Item = &'v Component> {
        static IMPLICIT_DEFAULT: Component = Component::Numeral(0);

        let version = self.version;

        // Create an iterator over all component
        let segment_components = (self.offset..self.offset + self.segment.len() as usize)
            .map(move |idx| &version.components[idx]);

        // Add an implicit default if this segment has one
        let implicit_default_component = self
            .segment
            .has_implicit_default()
            .then_some(&IMPLICIT_DEFAULT);

        // Join the two iterators together to get all the components of this segment.
        implicit_default_component
            .into_iter()
            .chain(segment_components)
    }
}

/// Version that only has equality when it is exactly the same
/// e.g for [`Version`] 1.0.0 == 1.0 while in [`StrictVersion`]
/// this is not equal. Useful in ranges where we are talking
/// about equality over version ranges instead of specific
/// version instances
#[derive(Clone, PartialOrd, Ord, Eq, Debug, Deserialize)]
pub struct StrictVersion(pub Version);

impl PartialEq for StrictVersion {
    fn eq(&self, other: &Self) -> bool {
        // StrictVersion is only equal if the number
        // of components are the same
        // and the components are the same
        self.0.components.len() == other.0.components.len() && self.0 == other.0
    }
}

impl Display for StrictVersion {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Hash for StrictVersion {
    fn hash<H: Hasher>(&self, state: &mut H) {
        fn hash_segments<'i, I: Iterator<Item = SegmentIter<'i>>, H: Hasher>(
            state: &mut H,
            segments: I,
        ) {
            for segment in segments {
                segment.components().rev().for_each(|c| c.hash(state));
            }
        }

        self.0.epoch().hash(state);
        hash_segments(state, self.0.segments());
        hash_segments(state, self.0.local_segments());
    }
}

#[cfg(test)]
mod test {
    use std::{
        cmp::Ordering,
        collections::hash_map::DefaultHasher,
        hash::{Hash, Hasher},
        str::FromStr,
    };

    use rand::seq::SliceRandom;
    use rstest::rstest;

    use super::{Component, Version};
    use crate::version::StrictVersion;

    // Tests are inspired by: https://github.com/conda/conda/blob/33a142c16530fcdada6c377486f1c1a385738a96/tests/models/test_version.py

    #[test]
    fn valid_versions() {
        enum CmpOp {
            Less,
            Equal,
            Restart,
        }

        let versions_str = [
            "   0.4",
            "== 0.4.0",
            " < 0.4.1.rc",
            "== 0.4.1.RC", // case-insensitive comparison
            " < 0.4.1",
            " < 0.5a1",
            " < 0.5b3",
            " < 0.5C1", // case-insensitive comparison
            " < 0.5",
            " < 0.9.6",
            " < 0.960923",
            " < 1.0",
            " < 1.1dev1", // special case 'dev'
            " < 1.1a1",
            " < 1.1.0dev1", // special case 'dev'
            "== 1.1.dev1",  // 0 is inserted before string
            " < 1.1.a1",
            " < 1.1.0rc1",
            " < 1.1.0",
            "== 1.1",
            " < 1.1.0post1", // special case 'post'
            "== 1.1.post1",  // 0 is inserted before string
            " < 1.1post1",   // special case 'post'
            " < 1996.07.12",
            " < 1!0.4.1", // epoch increased
            " < 1!3.1.1.6",
            " < 2!0.4.1", // epoch increased again
        ];

        let ops = versions_str.iter().map(|&v| {
            let (op, version_str) = if let Some((op, version)) = v.trim().split_once(' ') {
                (op, version.trim())
            } else {
                ("", v.trim())
            };
            let version: Version = version_str.parse().unwrap();
            let op = match op {
                "<" => CmpOp::Less,
                "==" => CmpOp::Equal,
                _ => CmpOp::Restart,
            };
            (op, version)
        });

        let mut previous: Option<Version> = None;
        for (op, version) in ops {
            match op {
                CmpOp::Less => {
                    let comparison = previous.as_ref().map(|previous| previous.cmp(&version));
                    assert!(
                        Some(Ordering::Less) == comparison,
                        "{} is not less than {}: {:?}",
                        previous
                            .as_ref()
                            .map(ToString::to_string)
                            .unwrap_or_default(),
                        version,
                        comparison
                    );
                }
                CmpOp::Equal => {
                    let comparison = previous.as_ref().map(|previous| previous.cmp(&version));
                    assert!(
                        Some(Ordering::Equal) == comparison,
                        "{} is not equal to {}: {:?}",
                        previous
                            .as_ref()
                            .map(ToString::to_string)
                            .unwrap_or_default(),
                        version,
                        comparison
                    );
                }
                CmpOp::Restart => {}
            }
            previous = Some(version);
        }
    }

    #[test]
    fn openssl_convention() {
        let version_strs = [
            "1.0.1dev",
            "1.0.1_", // <- this
            "1.0.1a",
            "1.0.1b",
            "1.0.1c",
            "1.0.1d",
            "1.0.1r",
            "1.0.1rc",
            "1.0.1rc1",
            "1.0.1rc2",
            "1.0.1s",
            "1.0.1", // <- compared to this
            "1.0.1post.a",
            "1.0.1post.b",
            "1.0.1post.z",
            "1.0.1post.za",
            "1.0.2",
        ];
        let parsed_versions: Vec<Version> =
            version_strs.iter().map(|v| v.parse().unwrap()).collect();
        let mut random_versions = parsed_versions.clone();
        random_versions.shuffle(&mut rand::thread_rng());
        random_versions.sort();
        assert_eq!(random_versions, parsed_versions);
    }

    #[test]
    fn test_pep440() {
        // this list must be in sorted order (slightly modified from the PEP 440 test
        // suite https://github.com/pypa/packaging/blob/master/tests/test_version.py)
        let versions = [
            // Implicit epoch of 0
            "1.0a1",
            "1.0a2.dev456",
            "1.0a12.dev456",
            "1.0a12",
            "1.0b1.dev456",
            "1.0b2",
            "1.0b2.post345.dev456",
            "1.0b2.post345",
            "1.0c1.dev456",
            "1.0c1",
            "1.0c3",
            "1.0rc2",
            "1.0.dev456",
            "1.0",
            "1.0.post456.dev34",
            "1.0.post456",
            "1.1.dev1",
            "1.2.r32+123456",
            "1.2.rev33+123456",
            "1.2+abc",
            "1.2+abc123def",
            "1.2+abc123",
            "1.2+123abc",
            "1.2+123abc456",
            "1.2+1234.abc",
            "1.2+123456",
            // Explicit epoch of 1
            "1!1.0a1",
            "1!1.0a2.dev456",
            "1!1.0a12.dev456",
            "1!1.0a12",
            "1!1.0b1.dev456",
            "1!1.0b2",
            "1!1.0b2.post345.dev456",
            "1!1.0b2.post345",
            "1!1.0c1.dev456",
            "1!1.0c1",
            "1!1.0c3",
            "1!1.0rc2",
            "1!1.0.dev456",
            "1!1.0",
            "1!1.0.post456.dev34",
            "1!1.0.post456",
            "1!1.1.dev1",
            "1!1.2.r32+123456",
            "1!1.2.rev33+123456",
            "1!1.2+abc",
            "1!1.2+abc123def",
            "1!1.2+abc123",
            "1!1.2+123abc",
            "1!1.2+123abc456",
            "1!1.2+1234.abc",
            "1!1.2+123456",
        ];

        let parsed_versions: Vec<Version> = versions.iter().map(|v| v.parse().unwrap()).collect();
        let mut random_versions = parsed_versions.clone();
        random_versions.shuffle(&mut rand::thread_rng());
        random_versions.sort();
        assert_eq!(random_versions, parsed_versions);
    }

    #[test]
    fn strict_version_test() {
        let v_1_0 = StrictVersion::from_str("1.0.0").unwrap();
        // Should be equal to itself
        assert_eq!(v_1_0, v_1_0);
        let v_1_0_0 = StrictVersion::from_str("1.0").unwrap();
        // Strict version should not discard zero's
        assert_ne!(v_1_0, v_1_0_0);
        // Ordering should stay the same as version
        assert_eq!(v_1_0.cmp(&v_1_0_0), Ordering::Equal);

        // Hashing should consider v_1_0 and v_1_0_0 as unequal
        assert_eq!(get_hash(&v_1_0), get_hash(&v_1_0));
        assert_ne!(get_hash(&v_1_0), get_hash(&v_1_0_0));
    }

    #[test]
    fn starts_with() {
        assert!(Version::from_str("1.2.3")
            .unwrap()
            .starts_with(&Version::from_str("1.2").unwrap()));
    }

    fn get_hash(spec: &impl Hash) -> u64 {
        let mut s = DefaultHasher::new();
        spec.hash(&mut s);
        s.finish()
    }

    #[test]
    fn hash() {
        let v1 = Version::from_str("1.2.0").unwrap();

        println!("{v1:?}");

        let vx2 = Version::from_str("1.2.0").unwrap();
        assert_eq!(get_hash(&v1), get_hash(&vx2));
        let vx2 = Version::from_str("1.2.0.0.0").unwrap();
        assert_eq!(get_hash(&v1), get_hash(&vx2));
        let vx2 = Version::from_str("1!1.2.0").unwrap();
        assert_ne!(get_hash(&v1), get_hash(&vx2));

        let vx2 = Version::from_str("1.2.0+post1").unwrap();
        assert_ne!(get_hash(&v1), get_hash(&vx2));

        let vx1 = Version::from_str("1.2+post1").unwrap();
        assert_eq!(get_hash(&vx1), get_hash(&vx2));

        let v2 = Version::from_str("1.2.3").unwrap();
        assert_ne!(get_hash(&v1), get_hash(&v2));
    }

    #[test]
    fn size_of_version() {
        assert_eq!(std::mem::size_of::<Version>(), 112);
    }

    #[test]
    fn as_major_minor() {
        assert_eq!(
            Version::from_str("1.2.3").unwrap().as_major_minor(),
            Some((1, 2))
        );
        assert_eq!(
            Version::from_str("5!1.2.3").unwrap().as_major_minor(),
            Some((1, 2))
        );
        assert_eq!(
            Version::from_str("1.2.3.5").unwrap().as_major_minor(),
            Some((1, 2))
        );
        assert_eq!(
            Version::from_str("1.2").unwrap().as_major_minor(),
            Some((1, 2))
        );
        assert_eq!(Version::from_str("1").unwrap().as_major_minor(), None);
        assert_eq!(Version::from_str("1a.2").unwrap().as_major_minor(), None);
        assert_eq!(Version::from_str("1.2a").unwrap().as_major_minor(), None);
        assert_eq!(
            Version::from_str("1.2.3a").unwrap().as_major_minor(),
            Some((1, 2))
        );
    }

    #[test]
    fn canonical() {
        assert_eq!(Version::from_str("1.2.3").unwrap().to_string(), "1.2.3");
        assert_eq!(Version::from_str("1!1.2.3").unwrap().to_string(), "1!1.2.3");
        assert_eq!(
            Version::from_str("1.2.3-alpha.2").unwrap().to_string(),
            "1.2.3-alpha.2"
        );
        assert_eq!(
            Version::from_str("1!1.2.3-alpha.2+3beta5rc")
                .unwrap()
                .to_string(),
            "1!1.2.3-alpha.2+3beta5rc"
        );
    }

    #[test]
    fn with_segments() {
        assert_eq!(
            Version::from_str("3!4.5a.6b+7.8")
                .unwrap()
                .with_segments(1..3)
                .unwrap(),
            Version::from_str("3!5a.6b+7.8").unwrap()
        );
        assert_eq!(
            Version::from_str("3!4.5a.6b+7.8")
                .unwrap()
                .with_segments(1..)
                .unwrap(),
            Version::from_str("3!5a.6b+7.8").unwrap()
        );
        assert_eq!(
            Version::from_str("3!4.5a.6b+7.8")
                .unwrap()
                .with_segments(..)
                .unwrap(),
            Version::from_str("3!4.5a.6b+7.8").unwrap()
        );
        assert_eq!(
            Version::from_str("0.11.0.post1+g1b5f1f6")
                .unwrap()
                .with_segments(..3)
                .unwrap(),
            Version::from_str("0.11.0+g1b5f1f6").unwrap()
        );
    }

    #[test]
    fn pop_segments() {
        assert_eq!(
            Version::from_str("3!4.5a.6b+7.8")
                .unwrap()
                .pop_segments(1)
                .unwrap(),
            Version::from_str("3!4.5a+7.8").unwrap()
        );
    }

    #[test]
    fn strip_local() {
        assert_eq!(
            Version::from_str("3!4.5a.6b+7.8")
                .unwrap()
                .strip_local()
                .into_owned(),
            Version::from_str("3!4.5a.6b").unwrap()
        );
    }

    #[rstest]
    #[case("1", 3, "1.0.0")]
    #[case("1.2", 3, "1.2.0")]
    #[case("1.2+3.4", 3, "1.2.0+3.4")]
    #[case("4!1.2+3.4", 3, "4!1.2.0+3.4")]
    #[case("4!1.2+3.4", 5, "4!1.2.0.0.0+3.4")]
    #[test]
    fn extend_to_length(#[case] version: &str, #[case] elements: usize, #[case] expected: &str) {
        assert_eq!(
            Version::from_str(version)
                .unwrap()
                .extend_to_length(elements)
                .unwrap()
                .to_string(),
            expected
        );
    }

    #[test]
    fn test_component_total_order() {
        // Create instances of each variant
        let components = vec![
            Component::Dev,
            Component::UnderscoreOrDash { is_dash: false },
            Component::Iden(Box::from("alpha")),
            Component::Iden(Box::from("beta")),
            Component::Numeral(1),
            Component::Numeral(2),
            Component::Post,
        ];

        // Check that each component equals itself
        for a in &components {
            assert_eq!(a.cmp(a), Ordering::Equal);
        }

        for (i, a) in components.iter().enumerate() {
            for b in components[i + 1..].iter() {
                let ord = a.cmp(b);
                assert_eq!(
                    ord,
                    Ordering::Less,
                    "Expected {a:?} < {b:?}, but found {ord:?}",
                );
            }
            // Check the reverse ordering as well
            // I think this should automatically check transitivity
            // If a <= b and b <= c, then a <= c
            for b in components[..i].iter() {
                let ord = a.cmp(b);
                assert_eq!(
                    ord,
                    Ordering::Greater,
                    "Expected {a:?} > {b:?}, but found {ord:?}",
                );
            }
        }
    }
}
