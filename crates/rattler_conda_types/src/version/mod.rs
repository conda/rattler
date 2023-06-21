use std::hash::{Hash, Hasher};
use std::{
    cmp::Ordering,
    fmt::{Debug, Display, Formatter},
    ops::Range,
};

use itertools::{EitherOrBoth, Itertools};
use serde::{Deserialize, Serialize, Serializer};
use smallvec::SmallVec;

pub use parse::{ParseVersionError, ParseVersionErrorKind};

mod parse;

/// This class implements an order relation between version strings. Version strings can contain the
/// usual alphanumeric characters (A-Za-z0-9), separated into components by dots and underscores.
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
#[derive(Clone, Debug, Eq, Deserialize)]
pub struct Version {
    /// A normed copy of the original version string trimmed and converted to lower case.
    /// Also dashes are replaced with underscores if the version string does not contain
    /// any underscores.
    norm: String,
    /// The version of this version. This is the actual version string, including the epoch.
    version: VersionComponent,
    /// The local version of this version (everything following a `+`).
    /// This is an optional string that can be used to indicate a local version of a package.
    local: VersionComponent,
}

impl Version {
    /// Tries to extract the major and minor versions from the version. Returns None if this instance
    /// doesnt appear to contain a major and minor version.
    pub fn as_major_minor(&self) -> Option<(usize, usize)> {
        self.version.as_major_minor()
    }

    /// Bumps this version to a version that is considered just higher than this version.
    pub fn bump(&self) -> Self {
        let mut result = self.clone();

        let last_component = result
            .version
            .components
            .iter_mut()
            .rev()
            .find_map(|component| match component {
                NumeralOrOther::Numeral(v) => Some(v),
                _ => None,
            });
        match last_component {
            Some(component) => *component += 1,
            None => unreachable!(),
        }

        result
    }

    /// Remove last element from version, e.g. 1.2.3 -> 1.2
    pub fn remove_last_element(&self) -> Self {
        let mut result = self.clone();

        result.version.components.pop();
        result.version.ranges.pop();

        result
    }

    /// Returns true if this is considered a dev version.
    pub fn is_dev(&self) -> bool {
        self.version.components.iter().any(|c| match c {
            NumeralOrOther::Other(name) => name == "DEV",
            _ => false,
        })
    }

    /// Check if this version version and local strings start with the same as other.
    pub fn starts_with(&self, other: &Self) -> bool {
        self.version.starts_with(&other.version) && self.local.starts_with(&other.local)
    }
}

impl PartialEq<Self> for Version {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version && self.local == other.local
    }
}

impl Hash for Version {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.version.hash(state);
        self.local.hash(state);
    }
}

/// Either a number, literal or the infinity.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
enum NumeralOrOther {
    Numeral(usize),
    Other(String),
    Infinity,
}

impl NumeralOrOther {
    pub fn as_number(&self) -> Option<usize> {
        match self {
            NumeralOrOther::Numeral(value) => Some(*value),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn as_string(&self) -> Option<&str> {
        match self {
            NumeralOrOther::Other(value) => Some(value.as_str()),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn is_infinity(&self) -> bool {
        matches!(self, NumeralOrOther::Infinity)
    }
}

impl From<usize> for NumeralOrOther {
    fn from(num: usize) -> Self {
        NumeralOrOther::Numeral(num)
    }
}

impl From<String> for NumeralOrOther {
    fn from(other: String) -> Self {
        NumeralOrOther::Other(other)
    }
}

impl Default for NumeralOrOther {
    fn default() -> Self {
        NumeralOrOther::Numeral(0)
    }
}

impl Ord for NumeralOrOther {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (NumeralOrOther::Other(_), NumeralOrOther::Numeral(_)) => Ordering::Less,
            (NumeralOrOther::Numeral(_), NumeralOrOther::Other(_)) => Ordering::Greater,
            (NumeralOrOther::Numeral(a), NumeralOrOther::Numeral(b)) => a.cmp(b),
            (NumeralOrOther::Other(a), NumeralOrOther::Other(b)) => a.cmp(b),
            (NumeralOrOther::Infinity, NumeralOrOther::Infinity) => Ordering::Equal,
            (NumeralOrOther::Infinity, _) => Ordering::Greater,
            (_, NumeralOrOther::Infinity) => Ordering::Less,
        }
    }
}

impl PartialOrd for NumeralOrOther {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Display for NumeralOrOther {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            NumeralOrOther::Numeral(n) => write!(f, "{}", n),
            NumeralOrOther::Other(s) => write!(f, "{}", s),
            NumeralOrOther::Infinity => write!(f, "∞"),
        }
    }
}

#[derive(Default, Clone, Eq, Serialize, Deserialize)]
struct VersionComponent {
    components: SmallVec<[NumeralOrOther; 4]>,
    ranges: SmallVec<[Range<usize>; 4]>,
}

impl VersionComponent {
    /// Tries to extract the major and minor versions from the version. Returns None if this instance
    /// doesnt appear to contain a major and minor version.
    pub fn as_major_minor(&self) -> Option<(usize, usize)> {
        match (self.range_as_number(1), self.range_as_number(2)) {
            (Some(major), Some(minor)) => Some((major, minor)),
            _ => None,
        }
    }

    /// Tries to convert the specified range to a number. Returns the number if possible; None otherwise.
    fn range_as_number(&self, range_idx: usize) -> Option<usize> {
        let range = self.ranges.get(range_idx)?;
        if range.end != range.start + 1 {
            return None;
        }
        let component = self.components.get(range.start)?;
        component.as_number()
    }

    pub(crate) fn starts_with(&self, other: &Self) -> bool {
        for ranges in self.ranges.iter().zip_longest(other.ranges.iter()) {
            let (left, right) = match ranges {
                EitherOrBoth::Both(left, right) => (left, right),
                EitherOrBoth::Left(_) => return true,
                EitherOrBoth::Right(_) => return false,
            };
            for values in left
                .clone()
                .map(|i| &self.components[i])
                .zip_longest(right.clone().map(|i| &other.components[i]))
            {
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
}

impl Hash for VersionComponent {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let default = NumeralOrOther::default();
        for range in self.ranges.iter().cloned() {
            // skip trailing default components
            self.components[range]
                .iter()
                .rev()
                .skip_while(|c| **c == default)
                .for_each(|c| c.hash(state));
        }
    }
}

impl Debug for VersionComponent {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "[")?;
        for (idx, range) in self.ranges.iter().cloned().enumerate() {
            if idx > 0 {
                write!(f, ", ")?;
            }
            write!(f, "[")?;
            for elems in itertools::Itertools::intersperse(
                self.components[range].iter().map(|c| format!("{}", c)),
                String::from(", "),
            ) {
                write!(f, "{}", elems)?;
            }
            write!(f, "]")?;
        }
        write!(f, "]")
    }
}

impl PartialEq for VersionComponent {
    fn eq(&self, other: &Self) -> bool {
        for ranges in self
            .ranges
            .iter()
            .cloned()
            .zip_longest(other.ranges.iter().cloned())
        {
            let (a_range, b_range) = ranges.or_default();
            let default = NumeralOrOther::default();
            for components in self.components[a_range]
                .iter()
                .zip_longest(other.components[b_range].iter())
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
}

impl Ord for VersionComponent {
    fn cmp(&self, other: &Self) -> Ordering {
        for ranges in self
            .ranges
            .iter()
            .cloned()
            .zip_longest(other.ranges.iter().cloned())
        {
            let (a_range, b_range) = ranges.or_default();
            for components in self.components[a_range]
                .iter()
                .zip_longest(other.components[b_range].iter())
            {
                let default = NumeralOrOther::default();
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
}

impl PartialOrd for VersionComponent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.version
            .cmp(&other.version)
            .then_with(|| self.local.cmp(&other.local))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Display for Version {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.norm.as_str())
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
}
