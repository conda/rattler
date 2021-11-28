use itertools::{EitherOrBoth, Itertools};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::num::ParseIntError;
use std::ops::Range;
use std::str::FromStr;

macro_rules! regex {
    ($re:literal $(,)?) => {{
        static RE: once_cell::sync::OnceCell<regex::Regex> = once_cell::sync::OnceCell::new();
        RE.get_or_init(|| regex::Regex::new($re).unwrap())
    }};
}

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

impl PartialOrd for NumeralOrOther {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
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

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct Version {
    norm: String,
    version: VersionComponent,
    local: VersionComponent,
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

#[derive(Debug)]
pub struct ParseVersionError {
    version: String,
    kind: ParseVersionKind,
}

impl Display for ParseVersionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed version string '{}': ", &self.version)?;
        match &self.kind {
            ParseVersionKind::Empty => write!(f, "empty string"),
            ParseVersionKind::InvalidCharacters => write!(f, "invalid character(s)"),
            ParseVersionKind::EpochMustBeInteger(e) => write!(f, "epoch must be an integer: {}", e),
            ParseVersionKind::DuplicateEpochSeparator => {
                write!(f, "duplicated epoch separator '!'")
            }
            ParseVersionKind::DuplicateLocalVersionSeparator => {
                write!(f, "duplicated local version separator '+'")
            }
            ParseVersionKind::EmptyVersionComponent => write!(f, "empty version component"),
            ParseVersionKind::InvalidNumeral(e) => write!(f, "invalid numeral: {}", e),
        }
    }
}

impl Error for ParseVersionError {}

impl ParseVersionError {
    pub fn new(text: impl Into<String>, kind: ParseVersionKind) -> Self {
        Self {
            version: text.into(),
            kind,
        }
    }
}

#[derive(Debug)]
pub enum ParseVersionKind {
    Empty,
    InvalidCharacters,
    EpochMustBeInteger(ParseIntError),
    InvalidNumeral(ParseIntError),
    DuplicateEpochSeparator,
    DuplicateLocalVersionSeparator,
    EmptyVersionComponent,
}

/// Returns true if the specified string contains only valid chars for a version string.
fn has_valid_chars(version: &str) -> bool {
    version
        .chars()
        .all(|c| matches!(c, '*'|'.'|'+'|'!'|'_'|'0'..='9'|'a'..='z'))
}

impl FromStr for Version {
    type Err = ParseVersionError;

    // Implementation taken from https://github.com/conda/conda/blob/0050c514887e6cbbc1774503915b45e8de12e405/conda/models/version.py#L47

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Version comparison is case-insensitive so normalize everything to lowercase
        let normalized = s.trim().to_lowercase();

        // Basic validity check
        if normalized.is_empty() {
            return Err(ParseVersionError::new(s, ParseVersionKind::Empty));
        }

        // Allow for dashes as long as there are no underscores as well. Dashes are then converted
        // to underscores.
        let lowered = if normalized.contains('-') && !normalized.contains('_') {
            normalized.replace('-', "_")
        } else {
            normalized
        };

        // Ensure the string only contains valid characters
        if !has_valid_chars(&lowered) {
            return Err(ParseVersionError::new(
                s,
                ParseVersionKind::InvalidCharacters,
            ));
        }

        // Find epoch
        let (epoch, rest) = if let Some((epoch, rest)) = lowered.split_once('!') {
            let _: usize = epoch
                .parse()
                .map_err(|e| ParseVersionError::new(s, ParseVersionKind::EpochMustBeInteger(e)))?;
            ([epoch], rest)
        } else {
            (["0"], lowered.as_str())
        };

        // Ensure the rest of the string no longer contains an epoch
        if rest.find('!').is_some() {
            return Err(ParseVersionError::new(
                s,
                ParseVersionKind::DuplicateEpochSeparator,
            ));
        }

        // Find local version string
        let (local, rest) = if let Some((rest, local)) = rest.rsplit_once('+') {
            (local, rest)
        } else {
            ("", rest)
        };

        // Ensure the rest of the string no longer contains a local version separator
        if rest.find('+').is_some() {
            return Err(ParseVersionError::new(
                s,
                ParseVersionKind::DuplicateLocalVersionSeparator,
            ));
        }

        // Split the local version by '_' or '.'
        let local_split = local.split(&['.', '_'][..]);

        // If the last character of a version is '-' or '_', don't split that out individually.
        // Implements the instructions for openssl-like versions. You can work-around this problem
        // by appending a dash to plain version numbers.
        let version: SmallVec<[String; 6]> = if rest.ends_with('_') {
            let mut versions: SmallVec<[String; 6]> = rest[..(rest.len() as isize - 1) as usize]
                .replace('_', ".")
                .split('.')
                .map(ToOwned::to_owned)
                .collect();
            if let Some(last) = versions.last_mut() {
                *last += "_";
            }
            versions
        } else {
            rest.replace('_', ".")
                .split('.')
                .map(ToOwned::to_owned)
                .collect()
        };
        let version_split = epoch
            .iter()
            .copied()
            .chain(version.iter().map(|s| s.as_str()));

        fn split_component<'a>(
            split_iter: impl Iterator<Item = &'a str>,
        ) -> Result<VersionComponent, ParseVersionKind> {
            let mut result = VersionComponent::default();
            for component in split_iter {
                let version_split_re = regex!(r#"([0-9]+|[^0-9]+)"#);
                let mut numeral_or_alpha_split = version_split_re.find_iter(component).peekable();
                if numeral_or_alpha_split.peek().is_none() {
                    return Err(ParseVersionKind::EmptyVersionComponent);
                }
                let range_start = result.components.len();
                for numeral_or_alpha in numeral_or_alpha_split {
                    let numeral_or_alpha = numeral_or_alpha.as_str();
                    let parsed: NumeralOrOther = match numeral_or_alpha {
                        num if num.chars().all(|c| c.is_ascii_digit()) => num
                            .parse::<usize>()
                            .map_err(ParseVersionKind::InvalidNumeral)?
                            .into(),
                        "post" => NumeralOrOther::Infinity,
                        "dev" => NumeralOrOther::Other(String::from("DEV")),
                        ident => NumeralOrOther::Other(ident.to_owned()),
                    };
                    result.components.push(parsed);
                }
                if range_start < result.components.len()
                    && !matches!(&result.components[range_start], NumeralOrOther::Numeral(_))
                {
                    result
                        .components
                        .insert(range_start, NumeralOrOther::Numeral(0))
                }

                let range_end = result.components.len();
                result.ranges.push(range_start..range_end);
            }
            Ok(result)
        }

        let version = split_component(version_split).map_err(|e| ParseVersionError::new(s, e))?;
        let local = if local.is_empty() {
            Default::default()
        } else {
            split_component(local_split).map_err(|e| ParseVersionError::new(s, e))?
        };

        Ok(Self {
            norm: lowered,
            version,
            local,
        })
    }
}

#[cfg(test)]
mod test {
    use crate::conda::Version;
    use rand::seq::SliceRandom;
    use std::cmp::Ordering;

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
}
