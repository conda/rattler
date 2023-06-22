use std::hash::{Hash, Hasher};
use std::{
    cmp::Ordering,
    fmt,
    fmt::{Debug, Display, Formatter},
    iter,
};

use itertools::{Either, EitherOrBoth, Itertools};
use serde::{Deserialize, Serialize, Serializer};
use smallvec::SmallVec;

pub use parse::{ParseVersionError, ParseVersionErrorKind};

mod parse;

/// Bitmask that should be applied to `Version::flags` to determine if the version contains an epoch.
const EPOCH_MASK: u8 = 0b00000001;

/// The bitmask to apply to `Version::flags` to get only the local version index.
const LOCAL_VERSION_MASK: u8 = !EPOCH_MASK;

/// The offset in bits where the the bits of the local version index start.
const LOCAL_VERSION_OFFSET: u8 = 1;

/// This class implements an order relation between version strings. Version strings can contain the
/// usual alphanumeric characters (A-Za-z0-9), separated into segments by dots and underscores.
/// Empty segments (i.e. two consecutive dots, a leading/trailing underscore) are not permitted. An
/// optional epoch number - an integer followed by '!' - can precede the actual version string (this
/// is useful to indicate a change in the versioning scheme itself). Version comparison is
/// case-insensitive.
///
/// Rattler supports six types of version strings:
///
/// * Release versions contain only integers, e.g. '1.0', '2.3.5'.
/// * Pre-release versions use additional letters such as 'a' or 'rc', for example '1.0a1',
///   '1.2.beta3', '2.3.5rc3'.
/// * Development versions are indicated by the string 'dev', for example '1.0dev42', '2.3.5.dev12'.
/// * Post-release versions are indicated by the string 'post', for example '1.0post1', '2.3.5.post2'.
/// * Tagged versions have a suffix that specifies a particular property of interest, e.g. '1.1.parallel'.
///   Tags can be added  to any of the preceding four types. As far as sorting is concerned,
///   tags are treated like strings in pre-release versions.
/// * An optional local version string separated by '+' can be appended to the main (upstream) version string.
///   It is only considered in comparisons when the main versions are equal, but otherwise handled in
///   exactly the same manner.
///
/// To obtain a predictable version ordering, it is crucial to keep the
/// version number scheme of a given package consistent over time.
///
/// Specifically,
///
/// * version strings should always have the same number of components (except for an optional tag suffix
///   or local version string),
/// * letters/strings indicating non-release versions should always occur at the same position.
///
/// Before comparison, version strings are parsed as follows:
///
/// * They are first split into epoch, version number, and local version number at '!' and '+' respectively.
///   If there is no '!', the epoch is set to 0. If there is no '+', the local version is empty.
/// * The version part is then split into components at '.' and '_'.
/// * Each component is split again into runs of numerals and non-numerals
/// * Subcomponents containing only numerals are converted to integers.
/// * Strings are converted to lower case, with special treatment for 'dev' and 'post'.
/// * When a component starts with a letter, the fillvalue 0 is inserted to keep numbers and strings in phase,
///   resulting in '1.1.a1' == 1.1.0a1'.
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
#[derive(Clone, Eq, Deserialize)]
pub struct Version {
    /// A normed copy of the original version string trimmed and converted to lower case.
    /// Also dashes are replaced with underscores if the version string does not contain
    /// any underscores.
    norm: Box<str>,

    /// Individual components of the version.
    ///
    /// We store a maximum of 3 components on the stack. If a version consists of more components
    /// they are stored on the heap instead. We choose 3 here because most versions only consist of
    /// 3 components.
    ///
    /// So for the version `1.2g.beta15.rc` this stores:
    ///
    /// [1, 2, 'g', 0, 'beta', 15, 0, 'rc']
    components: SmallVec<[Component; 3]>,

    /// The length of each individual segment. Segments group different components together.
    ///
    /// So for the version `1.2g.beta15.rc` this stores:
    ///
    /// [1,2,3,2]
    ///
    /// e.g. `1` consists of 1 component
    ///      `2g` consists of 2 components (`2` and `g`)
    ///      `beta15` consists of 3 components (`0`, `beta` and `15`). Segments must always start
    ///             with a number.
    ///      `rc` consists of 2 components (`0`, `rc`). Segments must always start with a number.
    segment_lengths: SmallVec<[u16; 4]>,

    /// Flags to indicate edge cases
    /// The first bit indicates whether or not this version has an epoch.
    /// The rest of the bits indicate from which segment the local version starts or 0 if there is
    /// no local version.
    flags: u8,
}

impl Version {
    /// Returns true if this version has an epoch.
    pub fn has_epoch(&self) -> bool {
        (self.flags & EPOCH_MASK) != 0
    }

    /// Returns true if this version has a local version defined
    pub fn has_local(&self) -> bool {
        ((self.flags & LOCAL_VERSION_MASK) >> LOCAL_VERSION_OFFSET) > 0
    }

    /// Returns the index of the first segment that belongs to the local version or `None` if there
    /// is no local version
    fn local_segment_index(&self) -> Option<usize> {
        let index = ((self.flags & LOCAL_VERSION_MASK) >> LOCAL_VERSION_OFFSET) as usize;
        if index > 0 {
            Some(index)
        } else {
            None
        }
    }

    /// Returns the epoch part of the version. If the version did not specify an epoch `0` is
    /// returned.
    pub fn epoch(&self) -> u64 {
        self.epoch_opt().unwrap_or(0)
    }

    /// Returns the epoch part of the version or `None` if the version did not specify an epoch.
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
    fn segments(
        &self,
    ) -> impl Iterator<Item = &'_ [Component]> + DoubleEndedIterator + ExactSizeIterator + '_ {
        let mut idx = if self.has_epoch() { 1 } else { 0 };
        let version_segments = if let Some(local_index) = self.local_segment_index() {
            &self.segment_lengths[..local_index]
        } else {
            &self.segment_lengths[..]
        };
        version_segments.iter().map(move |&count| {
            let start = idx;
            let end = idx + count as usize;
            idx += count as usize;
            &self.components[start..end]
        })
    }

    /// Returns a new version where the last numerical segment of this version has been bumped.
    pub fn bump(&self) -> Self {
        let mut bumped_version = self.clone();

        // Bump the last numeric components.
        let last_numeral = bumped_version
            .components
            .iter_mut()
            .rev()
            .find_map(|c| match c {
                Component::Numeral(num) => Some(num),
                _ => None,
            });

        match last_numeral {
            Some(last_numeral) => {
                *last_numeral += 1;
            }
            None => {
                // The only case when there is no numeral is when there is no epoch. So we just add
                // a 1 epoch.
                debug_assert!(!bumped_version.has_epoch());
                bumped_version.components.insert(0, Component::Numeral(1));
                bumped_version.flags |= EPOCH_MASK;
            }
        }

        // Update the normalized version string to reflect the changes
        bumped_version.norm = bumped_version.canonical().into_boxed_str();

        bumped_version
    }

    /// Returns the segments that belong the local part of the version.
    ///
    /// The local part of a a version is the part behind the (optional) `+`. E.g.:
    ///
    /// ```text
    /// 1.2+3.2.1-alpha0
    ///     ^^^^^^^^^^^^ This is the local part of the version
    /// ```
    fn local_segments(
        &self,
    ) -> impl Iterator<Item = &'_ [Component]> + DoubleEndedIterator + ExactSizeIterator + '_ {
        if let Some(start) = self.local_segment_index() {
            let mut idx = if self.has_epoch() { 1 } else { 0 };
            idx += self.segment_lengths[..start].iter().sum::<u16>() as usize;
            let version_segments = &self.segment_lengths[start..];
            Either::Left(version_segments.iter().map(move |&count| {
                let start = idx;
                let end = idx + count as usize;
                idx += count as usize;
                &self.components[start..end]
            }))
        } else {
            Either::Right(iter::empty())
        }
    }

    /// Tries to extract the major and minor versions from the version. Returns None if this instance
    /// doesnt appear to contain a major and minor version.
    pub fn as_major_minor(&self) -> Option<(u64, u64)> {
        let mut segments = self.segments();
        let major_segment = segments.next()?;
        let minor_segment = segments.next()?;

        if major_segment.len() == 1 && minor_segment.len() == 1 {
            Some((major_segment[0].as_number()?, minor_segment[0].as_number()?))
        } else {
            None
        }
    }

    /// Returns true if this is considered a dev version.
    ///
    /// If a version has a single component named "dev" it is considered to be a dev version.
    pub fn is_dev(&self) -> bool {
        self.segments()
            .flatten()
            .any(|component| component.as_string() == Some("dev"))
    }

    /// Check if this version version and local strings start with the same as other.
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

    /// Returns the canonical string representation of the version. This is all segments joined by dots.
    pub fn canonical(&self) -> String {
        fn format_components(components: &[Component]) -> impl Display {
            // Skip first component if its default and followed by a non-numeral
            let components = if components.len() > 1
                && components[0] == Component::default()
                && components[1].as_number().is_none()
            {
                &components[1..]
            } else {
                &components[..]
            };
            components.iter().join("")
        }

        fn format_segments<'i, I: Iterator<Item = &'i [Component]> + 'i>(
            segments: I,
        ) -> impl Display + 'i {
            segments.format_with(".", |components, f| f(&format_components(components)))
        }

        let epoch = self.epoch();
        let epoch_display = if epoch != 0 {
            format!("{}!", epoch)
        } else {
            format!("")
        };
        let segments_display = format_segments(self.segments());
        let local_display = if self.has_local() {
            format!("+{}", format_segments(self.local_segments()))
        } else {
            format!("")
        };

        format!("{}{}{}", epoch_display, segments_display, local_display)
    }
}

/// Returns true if the specified segments are considered to start with the other segments.
fn segments_starts_with<
    'a,
    'b,
    A: Iterator<Item = &'a [Component]> + 'a,
    B: Iterator<Item = &'b [Component]> + 'a,
>(
    a: A,
    b: B,
) -> bool {
    for ranges in a.zip_longest(b) {
        let (left, right) = match ranges {
            EitherOrBoth::Both(left, right) => (left, right),
            EitherOrBoth::Left(_) => return true,
            EitherOrBoth::Right(_) => return false,
        };
        for values in left.iter().zip_longest(right.iter()) {
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
        fn segments_equal<'i, I: Iterator<Item = &'i [Component]>>(a: I, b: I) -> bool {
            for ranges in a.zip_longest(b) {
                let (a_range, b_range) = ranges.or_default();
                let default = Component::default();
                for components in a_range.iter().zip_longest(b_range.iter()) {
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
        fn hash_segments<'i, I: Iterator<Item = &'i [Component]>, H: Hasher>(
            state: &mut H,
            segments: I,
        ) {
            let default = Component::default();
            for segment in segments {
                // The versions `1.0` and `1` are considered equal because a version has an infinite
                // number of default components in each segment. The get an equivalent hash we skip
                // trailing default components when computing the hash
                segment
                    .iter()
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

/// A helper function to display the segments of a [`Version`]
fn format_segments<'i, I: Iterator<Item = &'i [Component]>>(
    segments: I,
) -> impl fmt::Display + fmt::Debug {
    format!(
        "[{}]",
        segments.format_with(", ", |components, f| f(&format_args!(
            "[{}]",
            components.iter().format(", ")
        )))
    )
}

impl Debug for Version {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Version")
            .field("norm", &self.norm)
            .field(
                "version",
                &format_segments(
                    iter::once([Component::Numeral(self.epoch())].as_slice())
                        .chain(self.segments()),
                ),
            )
            .field("local", &format_segments(self.local_segments()))
            .finish()
    }
}

/// Either a number, literal or the infinity.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
enum Component {
    Numeral(u64),

    // Post should always be ordered greater than anything else.
    Post,

    // Dev should always be ordered less than anything else.
    Dev,

    // A generic string identifier. Identifiers are compared lexicographically. They are always
    // ordered less than numbers.
    Iden(Box<str>),
}

impl Component {
    pub fn as_number(&self) -> Option<u64> {
        match self {
            Component::Numeral(value) => Some(*value),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn as_string(&self) -> Option<&str> {
        match self {
            Component::Iden(value) => Some(value.as_ref()),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn is_post(&self) -> bool {
        matches!(self, Component::Post)
    }

    #[allow(dead_code)]
    pub fn is_dev(&self) -> bool {
        matches!(self, Component::Dev)
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
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            // Numbers are always ordered higher than strings
            (Component::Numeral(_), Component::Iden(_)) => Ordering::Greater,
            (Component::Iden(_), Component::Numeral(_)) => Ordering::Less,

            // Compare numbers and identifiers normally amongst themselves.
            (Component::Numeral(a), Component::Numeral(b)) => a.cmp(b),
            (Component::Iden(a), Component::Iden(b)) => a.cmp(b),
            (Component::Post, Component::Post) => Ordering::Equal,
            (Component::Dev, Component::Dev) => Ordering::Equal,

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
            Component::Numeral(n) => write!(f, "{}", n),
            Component::Iden(s) => write!(f, "{}", s),
            Component::Post => write!(f, "post"),
            Component::Dev => write!(f, "dev"),
        }
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        fn cmp_segments<'i, I: Iterator<Item = &'i [Component]>>(a: I, b: I) -> Ordering {
            for ranges in a.zip_longest(b) {
                let (a_range, b_range) = ranges.or_default();
                for components in a_range.iter().zip_longest(b_range.iter()) {
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
        f.write_str(self.norm.as_ref())
    }
}

impl Serialize for Version {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.norm)
    }
}

#[cfg(test)]
mod test {
    use std::cmp::Ordering;
    use std::str::FromStr;

    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use rand::seq::SliceRandom;

    use super::Version;

    // Tests are inspired by: https://github.com/conda/conda/blob/33a142c16530fcdada6c377486f1c1a385738a96/tests/models/test_version.py

    #[test]
    fn valid_versions() {
        let versions = [
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

        enum CmpOp {
            Less,
            Equal,
            Restart,
        }

        let ops = versions.iter().map(|&v| {
            let (op, version) = if let Some((op, version)) = v.trim().split_once(' ') {
                (op, version)
            } else {
                ("", v)
            };
            let version: Version = version.parse().unwrap();
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
                    if Some(Ordering::Less) != comparison {
                        panic!(
                            "{} is not less than {}: {:?}",
                            previous.as_ref().map(|v| v.to_string()).unwrap_or_default(),
                            version,
                            comparison
                        );
                    }
                }
                CmpOp::Equal => {
                    let comparison = previous.as_ref().map(|previous| previous.cmp(&version));
                    if Some(Ordering::Equal) != comparison {
                        panic!(
                            "{} is not equal to {}: {:?}",
                            previous.as_ref().map(|v| v.to_string()).unwrap_or_default(),
                            version,
                            comparison
                        );
                    }
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
        // this list must be in sorted order (slightly modified from the PEP 440 test suite
        // https://github.com/pypa/packaging/blob/master/tests/test_version.py)
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
    fn bump() {
        assert_eq!(
            Version::from_str("1.1").unwrap().bump(),
            Version::from_str("1.2").unwrap()
        );
        assert_eq!(
            Version::from_str("1.1l").unwrap().bump(),
            Version::from_str("1.2l").unwrap()
        )
    }

    #[test]
    fn starts_with() {
        assert!(Version::from_str("1.2.3")
            .unwrap()
            .starts_with(&Version::from_str("1.2").unwrap()));
    }

    fn get_hash(spec: &Version) -> u64 {
        let mut s = DefaultHasher::new();
        spec.hash(&mut s);
        s.finish()
    }

    #[test]
    fn hash() {
        let v1 = Version::from_str("1.2.0").unwrap();

        println!("{:?}", v1);

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
        assert_eq!(std::mem::size_of::<Version>(), 128);
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
        assert_eq!(Version::from_str("1.2.3").unwrap().canonical(), "1.2.3");
        assert_eq!(Version::from_str("1!1.2.3").unwrap().canonical(), "1!1.2.3");
        assert_eq!(
            Version::from_str("1.2.3-alpha.2").unwrap().canonical(),
            "1.2.3.alpha.2"
        );
        assert_eq!(
            Version::from_str("1!1.2.3-alpha.2+3beta5rc")
                .unwrap()
                .canonical(),
            "1!1.2.3.alpha.2+3beta5rc"
        );
    }
}
