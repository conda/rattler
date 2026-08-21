//! Types and operations for conda version literals as specified by
//! [CEP 33](https://conda.org/learn/ceps/cep-0033).

use std::{
    borrow::Cow,
    cell::RefCell,
    cmp::Ordering,
    collections::Bound,
    fmt,
    fmt::{Debug, Display, Formatter},
    hash::{Hash, Hasher},
    iter,
    ops::{Deref, DerefMut, RangeBounds},
};

use itertools::{Either, EitherOrBoth, Itertools};
pub use parse::{ParseVersionError, ParseVersionErrorKind};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
use smallvec::SmallVec;

mod flags;
pub(crate) mod parse;
mod segment;
#[cfg(feature = "semver")]
mod semver;
mod with_source;

pub(crate) mod bump;
pub use bump::{VersionBumpError, VersionBumpType};
use flags::Flags;
use segment::Segment;
// Disambiguated from the `semver` crate, which is used by the module itself.
#[cfg(feature = "semver")]
pub use self::semver::VersionToSemverError;
use thiserror::Error;
pub use with_source::VersionWithSource;

/// A parsed conda package version literal.
///
/// `Version` implements the literal parsing and ordering specified by
/// [CEP 33](https://conda.org/learn/ceps/cep-0033), including epochs, local
/// versions, arbitrary alphanumeric components, and the special `dev` and
/// `post` identifiers. Equality is normalized version-literal equality:
/// missing components compare as `0`, so `1.0 == 1.0.0`.
///
/// # Parsing and normalization
///
/// A literal consists of an optional `epoch!`, release segments, and an
/// optional local part after `+`. Release and local parts are split into
/// segments at `.`, `_`, or the historically accepted `-`; each segment is
/// split into numeric and textual components. Numeric components lose leading
/// zeroes, textual components are lowercased, and a segment starting with text
/// receives an implicit leading `0`. The `dev` and `post` components have their
/// special ordering behavior.
///
/// Parsing retains the version model—its epoch, release and local segments,
/// segment boundaries, component kinds, and separators—but not its exact
/// spelling. [`StrictVersion`] and [`VersionSpec`][crate::VersionSpec] use the
/// retained structure for prefix and compatible constraints. Use [`VersionWithSource`] when the
/// original text must be retained for display or serialization.
///
/// ```
/// # use rattler_conda_version::Version;
/// # use std::str::FromStr;
/// let release = Version::from_str("1.0").unwrap();
/// let candidate = Version::from_str("1.0rc1").unwrap();
///
/// assert!(candidate < release);
/// assert_eq!(release, Version::from_str("1.0.0").unwrap());
/// ```
///
/// # Optional SemVer interoperability
///
/// With the `semver` feature enabled, [`Version`] converts from
/// `semver::Version`, and can be fallibly converted back. The latter may fail
/// because conda version literals can express forms that SemVer cannot.
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

/// Explains why a [`Version`] could not be extended with zero-valued segments.
#[derive(Error, Debug, PartialEq)]

pub enum VersionExtendError {
    /// Adding segments would exceed the representation's maximum segment count.
    #[error("the version is too long")]
    VersionTooLong,
}

impl Version {
    /// Creates a release-style [`Version`] containing only its first numeric segment.
    ///
    /// ```
    /// # use rattler_conda_version::Version;
    /// assert_eq!(Version::major(2).to_string(), "2");
    /// ```
    pub fn major(major: u64) -> Version {
        Version {
            components: smallvec::smallvec![Component::Numeral(major)],
            segments: smallvec::smallvec![Segment::new(1).unwrap()],
            flags: Flags(0),
        }
    }

    /// Reports whether this [`Version`] was written with an epoch such as `1!2.0`.
    pub fn has_epoch(&self) -> bool {
        self.flags.has_epoch()
    }

    /// Reports whether this [`Version`] has a local version following `+`.
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

    /// Returns this [`Version`]'s epoch, or `0` when no epoch was written.
    pub fn epoch(&self) -> u64 {
        self.epoch_opt().unwrap_or(0)
    }

    /// Returns this [`Version`]'s explicit epoch, if it has one.
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

    /// Iterates the release segments of this [`Version`], excluding its local part.
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

    /// Iterates the local segments of this [`Version`].
    ///
    /// The local part is the portion after `+`, such as `3.2.1-alpha0` in `1.2+3.2.1-alpha0`.
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

    /// Returns the first two numeric release segments when this [`Version`] has a simple major-minor form.
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

    /// Reports whether this [`Version`] contains conda's special `dev` component.
    pub fn is_dev(&self) -> bool {
        self.segments()
            .flat_map(|segment| segment.components())
            .any(Component::is_dev)
    }

    /// Reports whether this [`Version`] contains conda's special `post` component.
    pub fn is_post(&self) -> bool {
        self.segments()
            .flat_map(|segment| segment.components())
            .any(Component::is_post)
    }

    /// Reports whether this [`Version`] has `other` as a conda-version prefix.
    pub fn starts_with(&self, other: &Self) -> bool {
        self.epoch() == other.epoch()
            && segments_starts_with(self.segments(), other.segments())
            && segments_starts_with(self.local_segments(), other.local_segments())
    }

    /// Reports whether this [`Version`] satisfies conda's compatible-version relation with `other`.
    pub fn compatible_with(&self, other: &Self) -> bool {
        self.ge(other)
            && self.epoch() == other.epoch()
            // Remove the last segment from the limit.
            && segments_starts_with(self.segments(), other.segments().rev().skip(1).rev())
            // Local version comparison remains the same
            && segments_starts_with(self.local_segments(), other.local_segments())
    }

    /// Returns a [`Version`] made from the selected release segments of this [`Version`].
    ///
    /// The local part is retained. Returns `None` when the range is empty or
    /// falls outside the release segments.
    ///
    /// ```
    /// # use rattler_conda_version::Version;
    /// # use std::str::FromStr;
    /// let version = Version::from_str("1.3a.4-alpha3+build").unwrap();
    /// let selected = version.with_segments(1..3).unwrap();
    ///
    /// assert_eq!(selected.to_string(), "3a.4+build");
    /// ```
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

    /// Removes `n` release segments, returning `None` if no release segment would remain.
    ///
    /// The local part is retained when the operation succeeds.
    pub fn pop_segments(&self, n: usize) -> Option<Version> {
        let segment_count = self.segment_count();
        if segment_count < n {
            None
        } else {
            self.with_segments(..segment_count - n)
        }
    }

    /// Returns the number of release segments in this [`Version`], excluding its local part.
    pub fn segment_count(&self) -> usize {
        if let Some(local_index) = self.local_segment_index() {
            local_index
        } else {
            self.segments.len()
        }
    }

    /// Returns this [`Version`] without its local part, borrowing when none is present.
    ///
    /// For example, `1.0+build.1` becomes `1.0`.
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

    /// Extends this [`Version`] with zero-valued release segments to reach `length`.
    ///
    /// Returns a borrowed value when this [`Version`] already has at least
    /// `length` release segments. The local part is preserved.
    ///
    /// ```
    /// # use rattler_conda_version::Version;
    /// # use std::str::FromStr;
    /// let version = Version::from_str("1.2+build").unwrap();
    /// let extended = version.extend_to_length(3).unwrap().into_owned();
    ///
    /// assert_eq!(extended, Version::from_str("1.2.0+build").unwrap());
    /// ```
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
                // The segment retains whether its leading zero is implicit, so
                // do not store that generated component a second time.
                let implicit_default = usize::from(segment_iter.has_implicit_default());
                for component in segment_iter.components().skip(implicit_default).cloned() {
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
    b: B, // the prefix we're looking for in 'a'
) -> bool {
    let mut had_extra_left = false;
    for ranges in a.zip_longest(b) {
        let (left, right) = match ranges {
            EitherOrBoth::Both(left, right) => {
                // Previous segment had extra left components, but there are more
                // prefix segments - this is a structural mismatch.
                // E.g., "1.1c.1" does NOT start with "1.1.1"
                if had_extra_left {
                    return false;
                }
                (left, right)
            }
            // Extra segments in version after prefix is exhausted - OK
            // E.g., "1.0.1.2" starts with "1.0.1"
            EitherOrBoth::Left(_) => return true,
            EitherOrBoth::Right(segment) => {
                // Prefix has more segments. Zero segments are OK (implicit zeros).
                // E.g., "1.0" starts with "1.0.0"
                if segment.is_zero() {
                    continue;
                }
                return false;
            }
        };
        had_extra_left = false;
        for values in left.components().zip_longest(right.components()) {
            match values {
                EitherOrBoth::Both(a, b) => {
                    if a != b {
                        return false;
                    }
                }
                // Extra components in version segment. Only OK if this is the last
                // prefix segment (checked on next outer iteration).
                // E.g., "1.0.1c" starts with "1.0.1"
                EitherOrBoth::Left(_) => {
                    had_extra_left = true;
                    break;
                }
                // Missing component in version. Zero components are OK (implicit zeros).
                // E.g., "1.1c.1" starts with "1.1c0.1"
                EitherOrBoth::Right(component) => {
                    if component.is_zero() {
                        continue;
                    }
                    return false;
                }
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

/// A single ordered component within a parsed [`Version`].
///
/// Components represent numeric and textual parts of a segment, plus conda's
/// special `dev` and `post` values used during version comparison.
#[derive(Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum Component {
    /// A numeric component such as the `2` in `1.2`.
    Numeral(u64),

    /// Conda's `post` component, ordered after every other component.
    Post,

    /// Conda's `dev` component, ordered before every other component.
    Dev,

    /// A textual identifier, ordered lexicographically before numeric components.
    Iden(Box<str>),

    /// A separator retained as a component for conda's ordering rules.
    UnderscoreOrDash {
        /// Whether the retained separator is `-` rather than `_`.
        is_dash: bool,
    },
}

impl Component {
    /// Returns the numeric value when this [`Component`] is [`Component::Numeral`].
    pub fn as_number(&self) -> Option<u64> {
        match self {
            Component::Numeral(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns the mutable numeric value when this [`Component`] is [`Component::Numeral`].
    pub fn as_number_mut(&mut self) -> Option<&mut u64> {
        match self {
            Component::Numeral(value) => Some(value),
            _ => None,
        }
    }

    /// Returns the textual identifier when this [`Component`] is [`Component::Iden`].
    pub fn as_iden(&self) -> Option<&str> {
        match self {
            Component::Iden(value) => Some(value),
            _ => None,
        }
    }

    /// Returns the mutable textual identifier when this [`Component`] is [`Component::Iden`].
    pub fn as_iden_mut(&mut self) -> Option<&mut Box<str>> {
        match self {
            Component::Iden(value) => Some(value),
            _ => None,
        }
    }

    /// Returns the textual identifier as a string slice, if this is [`Component::Iden`].
    #[allow(dead_code)]
    pub fn as_string(&self) -> Option<&str> {
        match self {
            Component::Iden(value) => Some(value.as_ref()),
            _ => None,
        }
    }

    /// Reports whether this component is conda's special [`Component::Post`] value.
    #[allow(dead_code)]
    pub fn is_post(&self) -> bool {
        matches!(self, Component::Post)
    }

    /// Reports whether this component is conda's special [`Component::Dev`] value.
    #[allow(dead_code)]
    pub fn is_dev(&self) -> bool {
        matches!(self, Component::Dev)
    }

    /// Reports whether this component is a numeric [`Component::Numeral`].
    pub fn is_numeric(&self) -> bool {
        matches!(self, Component::Numeral(_))
    }

    /// Reports whether this component is the numeric value `0`.
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

            // Compare numbers and identifiers normally among themselves.
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

/// A view of one release or local segment within a parsed [`Version`].
pub struct SegmentIter<'v> {
    /// Internal metadata that identifies the segment.
    segment: Segment,

    /// Position of this segment's first stored component.
    offset: usize,

    /// Version that owns this segment.
    version: &'v Version,
}

impl<'v> SegmentIter<'v> {
    /// Reports whether every component in this segment is numeric zero.
    pub fn is_zero(&self) -> bool {
        self.components().all(Component::is_zero)
    }

    /// Reports whether conda inserted an implicit leading zero while parsing this segment.
    ///
    /// The inserted value keeps letter-led segments comparable with numeric segments;
    /// for example, `2.a` is represented as `2.0a`.
    pub fn has_implicit_default(&self) -> bool {
        self.segment.has_implicit_default()
    }

    /// Returns the separator before this segment, or `None` for the first segment.
    pub fn separator(&self) -> Option<char> {
        self.segment.separator()
    }

    /// Returns this segment's stored component count, excluding an implicit leading zero.
    pub fn component_count(&self) -> usize {
        self.segment.len() as usize
    }

    /// Iterates this segment's components, including an implicit leading zero when present.
    pub fn components(&self) -> impl DoubleEndedIterator<Item = &'v Component> + use<'v> {
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

/// A [`Version`] wrapper that distinguishes structurally different but
/// normalization-equivalent versions.
///
/// Unlike [`Version`], a `StrictVersion` parsed from `1.0` and one parsed
/// from `1.0.0` are not equal. [`crate::VersionSpec`] uses it for operators whose behavior depends
/// on the written version structure.
///
/// The wrapped [`Version`] is private so callers cannot accidentally depend on
/// the wrapper's representation. Use [`Deref`], [`AsRef`], or [`Into`] to work
/// with the underlying version.
#[repr(transparent)]
#[derive(Clone, Eq, Debug, Deserialize)]
pub struct StrictVersion(Version);

impl StrictVersion {
    /// Wraps `version` with structural equality.
    pub fn new(version: Version) -> Self {
        Self(version)
    }

    /// Returns the wrapped [`Version`], consuming the wrapper.
    pub fn into_inner(self) -> Version {
        self.0
    }
}

impl From<Version> for StrictVersion {
    fn from(version: Version) -> Self {
        Self::new(version)
    }
}

impl From<StrictVersion> for Version {
    fn from(version: StrictVersion) -> Self {
        version.into_inner()
    }
}

impl AsRef<Version> for StrictVersion {
    fn as_ref(&self) -> &Version {
        &self.0
    }
}

impl AsMut<Version> for StrictVersion {
    fn as_mut(&mut self) -> &mut Version {
        &mut self.0
    }
}

impl Deref for StrictVersion {
    type Target = Version;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl DerefMut for StrictVersion {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}

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

impl Ord for StrictVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        // Use Version's semantic ordering as the primary key, then break ties
        // using the raw component count.
        self.0
            .cmp(&other.0)
            .then_with(|| self.0.components.len().cmp(&other.0.components.len()))
    }
}

impl PartialOrd for StrictVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
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
        random_versions.shuffle(&mut rand::rng());
        random_versions.sort();
        assert_eq!(random_versions, parsed_versions);
    }

    #[test]
    fn dev_is_only_special_for_exact_runs() {
        // Because 1.2.devdev is not considered a dev version, it must sort after 1.2dev
        assert!(Version::from_str("1.2dev").unwrap() < Version::from_str("1.2.devdev").unwrap());
        assert!(Version::from_str("1.2dev").unwrap() < Version::from_str("1.2devdev").unwrap());
        // Same with post
        assert!(Version::from_str("1.2postpost").unwrap() < Version::from_str("1.2post").unwrap());

        // 1.2dev is a dev version, but 1.2devdev is not.
        assert!(Version::from_str("1.2dev").unwrap().is_dev());
        assert!(!Version::from_str("1.2devdev").unwrap().is_dev());
        assert!(!Version::from_str("1.2.devdev").unwrap().is_dev());

        // 1.2post is a post version, but 1.2postpost is not.
        assert!(Version::from_str("1.2post").unwrap().is_post());
        assert!(!Version::from_str("1.2postpost").unwrap().is_post());
        assert!(!Version::from_str("1.2.postpost").unwrap().is_post());
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
        random_versions.shuffle(&mut rand::rng());
        random_versions.sort();
        assert_eq!(random_versions, parsed_versions);
    }

    #[test]
    fn strict_version_accessors() {
        let version = Version::from_str("1.0").unwrap();
        let mut strict = StrictVersion::from(version.clone());

        assert_eq!(strict.as_ref(), &version);
        let _: &Version = &strict;
        let _: &mut Version = &mut strict;

        let extracted: Version = strict.into();
        assert_eq!(extracted, version);
    }

    #[test]
    fn strict_version_test() {
        let v_1_0_0 = StrictVersion::from_str("1.0.0").unwrap();
        // Should be equal to itself
        assert_eq!(v_1_0_0, v_1_0_0);
        let v_1_0 = StrictVersion::from_str("1.0").unwrap();
        // Strict version should not discard trailing zeros
        assert_ne!(v_1_0_0, v_1_0);

        // Hashing should consider v_1_0_0 and v_1_0 as unequal
        assert_eq!(get_hash(&v_1_0_0), get_hash(&v_1_0_0));
        assert_ne!(get_hash(&v_1_0_0), get_hash(&v_1_0));
    }

    /// Regression test: `StrictVersion::cmp` must not return `Equal` for
    /// versions that `StrictVersion::eq` considers different.
    #[test]
    fn strict_version_ord_contract() {
        let v100 = StrictVersion::from_str("1.0.0").unwrap();
        let v10 = StrictVersion::from_str("1.0").unwrap();

        // PartialEq: distinct strict versions
        assert_ne!(v100, v10);

        // Ord: must not return Equal for unequal values
        assert_ne!(v100.cmp(&v10), Ordering::Equal);
        assert_ne!(v10.cmp(&v100), Ordering::Equal);

        // Ordering must be antisymmetric
        assert_eq!(v10.cmp(&v100), Ordering::Less);
        assert_eq!(v100.cmp(&v10), Ordering::Greater);

        // Reflexivity
        assert_eq!(v100.cmp(&v100), Ordering::Equal);
        assert_eq!(v10.cmp(&v10), Ordering::Equal);

        // BTreeSet must hold both as distinct entries
        let mut set = std::collections::BTreeSet::new();
        set.insert(v10.clone());
        set.insert(v100.clone());
        assert_eq!(set.len(), 2, "BTreeSet lost one entry due to Ord violation");

        // Sort and dedup must preserve both entries
        let mut vec = vec![v100.clone(), v10.clone(), v100.clone()];
        vec.sort();
        vec.dedup();
        assert_eq!(vec.len(), 2, "dedup collapsed distinct strict versions");

        // Semantic comparison via the inner Version is still Equal
        assert_eq!(v100.0.cmp(&v10.0), Ordering::Equal);
    }

    #[test]
    fn strict_version_ord_with_genuine_differences() {
        // Versions that are genuinely less/greater should order correctly
        let cases: &[(&str, &str)] = &[
            ("1.0", "2.0"),
            ("1.0.0", "2.0.0"),
            ("1.0", "1.1"),
            ("1.0.0", "1.0.1"),
        ];
        for (lesser, greater) in cases {
            let a = StrictVersion::from_str(lesser).unwrap();
            let b = StrictVersion::from_str(greater).unwrap();
            assert_eq!(
                a.cmp(&b),
                Ordering::Less,
                "{lesser} should be Less than {greater}"
            );
            assert_eq!(b.cmp(&a), Ordering::Greater);
        }
    }

    #[test]
    fn starts_with() {
        assert!(
            Version::from_str("1.2.3")
                .unwrap()
                .starts_with(&Version::from_str("1.2").unwrap())
        );
    }

    #[test]
    fn starts_with_differing_component_sizes() {
        // segments: [2, 0, 1, [version, 2]]
        let version = Version::from_str("2.0.1.version2").unwrap();
        // segments: [2, 0, 1, version, 3]
        let other_version = Version::from_str("2.0.1.version.3").unwrap();

        assert!(!version.starts_with(&other_version));
        assert!(!other_version.starts_with(&version));
    }

    #[test]
    fn starts_with_extra_components() {
        // For glob matching (e.g., "1.0.0_version*"), versions with extra
        // components should match the prefix.
        let version = Version::from_str("1.0.0_version").unwrap();
        // "1.0.0_version1" starts with "1.0.0_version" (extra component "1")
        assert!(
            Version::from_str("1.0.0_version1")
                .unwrap()
                .starts_with(&version)
        );
        // "1.0.0_version0" starts with "1.0.0_version" (extra component "0")
        assert!(
            Version::from_str("1.0.0_version0")
                .unwrap()
                .starts_with(&version)
        );
        // "1.0.0_version0foo" starts with "1.0.0_version" (extra components "0", "foo")
        assert!(
            Version::from_str("1.0.0_version0foo")
                .unwrap()
                .starts_with(&version)
        );

        // But different base components should NOT match
        assert!(
            !Version::from_str("1.0.0_other")
                .unwrap()
                .starts_with(&version)
        );
        assert!(
            !Version::from_str("1.0.0_ver")
                .unwrap()
                .starts_with(&version)
        );

        // Different segment structure should NOT match (PR #1791)
        // "1.0.0_version1" has segment [version, 1]
        // "1.0.0_version_2" has segments [version], [2]
        // This ensures "1.0.0_version_2*" does NOT match "1.0.0_version1"
        let version_with_extra_segment = Version::from_str("1.0.0_version_2").unwrap();
        assert!(
            !Version::from_str("1.0.0_version1")
                .unwrap()
                .starts_with(&version_with_extra_segment)
        );

        // Different component values should NOT match
        let version1 = Version::from_str("1.0.0_version1").unwrap();
        assert!(
            !Version::from_str("1.0.0_version0")
                .unwrap()
                .starts_with(&version1)
        );

        // Extra components after matching prefix should match
        assert!(
            Version::from_str("1.0.0_version1a")
                .unwrap()
                .starts_with(&version1)
        );
        assert!(
            Version::from_str("1.0.0_version0a")
                .unwrap()
                .starts_with(&version)
        );

        // Extra components in INTERMEDIATE segments should NOT match
        // "1.1c.1" has segment [1, c] where "1.1.1" has segment [1]
        // This is a structure mismatch, not just extra components at the end
        assert!(
            !Version::from_str("1.1c.1")
                .unwrap()
                .starts_with(&Version::from_str("1.1.1").unwrap())
        );
        assert!(
            !Version::from_str("1.1c1.1")
                .unwrap()
                .starts_with(&Version::from_str("1.1c.1").unwrap())
        );

        // BUT zero components in prefix are treated as "no component" (implicit zeros)
        // So "1.1c.1" starts with "1.1c0.1" because c0 == c (trailing zero)
        assert!(
            Version::from_str("1.1c.1")
                .unwrap()
                .starts_with(&Version::from_str("1.1c0.1").unwrap())
        );
    }

    /// Test for <https://github.com/conda/rattler/issues/1914>
    /// Versions with letter suffixes (like openssl 1.0.1c) should match
    /// prefix patterns (like 1.0.1*).
    #[test]
    fn starts_with_letter_suffix() {
        // openssl versions like 1.0.1c, 1.0.1g, etc. should start with 1.0.1
        let prefix = Version::from_str("1.0.1").unwrap();
        assert!(Version::from_str("1.0.1c").unwrap().starts_with(&prefix));
        assert!(Version::from_str("1.0.1g").unwrap().starts_with(&prefix));
        assert!(Version::from_str("1.0.1k").unwrap().starts_with(&prefix));

        // Also test 1.0.2 series
        let prefix_2 = Version::from_str("1.0.2").unwrap();
        assert!(Version::from_str("1.0.2a").unwrap().starts_with(&prefix_2));
        assert!(Version::from_str("1.0.2l").unwrap().starts_with(&prefix_2));

        // Negative cases - these should NOT match
        assert!(!Version::from_str("1.0.2a").unwrap().starts_with(&prefix));
        assert!(!Version::from_str("1.0.1c").unwrap().starts_with(&prefix_2));
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
    #[case("1.2+build", 3, "1.2.0+build")]
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
