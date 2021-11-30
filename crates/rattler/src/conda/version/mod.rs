mod parse;

use itertools::{EitherOrBoth, Itertools};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::{
    fmt::{Debug, Display, Formatter},
    cmp::Ordering,
    ops::Range
};

pub use parse::{ParseVersionErrorKind, ParseVersionError};

/// This class implements an order relation between version strings. Version strings can contain the
/// usual alphanumeric characters (A-Za-z0-9), separated into components by dots and underscores.
/// Empty segments (i.e. two consecutive dots, a leading/trailing underscore) are not permitted. An
/// optional epoch number - an integer followed by '!' - can precede the actual version string (this
/// is useful to indicate a change in the versioning scheme itself). Version comparison is
/// case-insensitive.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct Version {
    norm: String,
    version: VersionComponent,
    local: VersionComponent,
}

impl Version {
    /// Bumps this version to a version that is considered just higher than this version.
    pub fn bump(&self) -> Self {
        let mut result = self.clone();

        if let NumeralOrOther::Numeral(num) = result.version.components.last_mut().expect("there must be at least one component") {
            *num += 1;
        } else {
            result.version.components.push(NumeralOrOther::Numeral(1));
            let last_range = result.version.ranges.last_mut().expect("there must be at least one range");
            last_range.end += 1;
        }

        result
    }

    /// Returns true if this is considered a dev version.
    pub fn is_dev(&self) -> bool {
        self.version.components.iter().any(|c| match c {
            NumeralOrOther::Other(name) => name == "DEV",
            _ => false,
        })
    }
}

/// Either a number, literal or the infinity.
#[derive(Debug, Clone, Eq, PartialEq, Hash, derive_more::From, Serialize, Deserialize)]
enum NumeralOrOther {
    Numeral(usize),
    Other(String),
    Infinity,
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

#[derive(Default, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
struct VersionComponent {
    components: SmallVec<[NumeralOrOther; 4]>,
    ranges: SmallVec<[Range<usize>; 4]>,
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

#[cfg(test)]
mod test {
    use crate::conda::Version;
    use rand::seq::SliceRandom;
    use std::cmp::Ordering;
    use std::str::FromStr;

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
    fn openssl_convetion() {
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
        Version::from_str("1")
    }
}
