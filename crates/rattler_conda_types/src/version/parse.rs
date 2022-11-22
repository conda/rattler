use super::{NumeralOrOther, Version, VersionComponent};
use smallvec::SmallVec;
use std::{
    convert::Into,
    default::Default,
    error::Error,
    fmt::{Display, Formatter},
    num::ParseIntError,
    result::Result,
    str::FromStr,
};

/// An error that occurred during parsing of a string to a version.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ParseVersionError {
    /// The original string that was the input of the parser
    pub version: String,

    /// The type of parse error that occurred
    pub kind: ParseVersionErrorKind,
}

impl Display for ParseVersionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed version string '{}': ", &self.version)?;
        match &self.kind {
            ParseVersionErrorKind::Empty => write!(f, "empty string"),
            ParseVersionErrorKind::InvalidCharacters => write!(f, "invalid character(s)"),
            ParseVersionErrorKind::EpochMustBeInteger(e) => {
                write!(f, "epoch must be an integer: {}", e)
            }
            ParseVersionErrorKind::DuplicateEpochSeparator => {
                write!(f, "duplicated epoch separator '!'")
            }
            ParseVersionErrorKind::DuplicateLocalVersionSeparator => {
                write!(f, "duplicated local version separator '+'")
            }
            ParseVersionErrorKind::EmptyVersionComponent => write!(f, "empty version component"),
            ParseVersionErrorKind::InvalidNumeral(e) => write!(f, "invalid numeral: {}", e),
        }
    }
}

impl Error for ParseVersionError {}

impl ParseVersionError {
    pub fn new(text: impl Into<String>, kind: ParseVersionErrorKind) -> Self {
        Self {
            version: text.into(),
            kind,
        }
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum ParseVersionErrorKind {
    Empty,
    InvalidCharacters,
    EpochMustBeInteger(ParseIntError),
    InvalidNumeral(ParseIntError),
    DuplicateEpochSeparator,
    DuplicateLocalVersionSeparator,
    EmptyVersionComponent,
}

/// Returns true if the specified char is a valid char for a version string.
pub(crate) fn is_valid_char(c: char) -> bool {
    matches!(c, '.'|'+'|'!'|'_'|'0'..='9'|'a'..='z')
}

/// Returns true if the specified string contains only valid chars for a version string.
fn has_valid_chars(version: &str) -> bool {
    version.chars().all(is_valid_char)
}

impl FromStr for Version {
    type Err = ParseVersionError;

    // Implementation taken from https://github.com/conda/conda/blob/0050c514887e6cbbc1774503915b45e8de12e405/conda/models/version.py#L47

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Version comparison is case-insensitive so normalize everything to lowercase
        let normalized = s.trim().to_lowercase();

        // Basic validity check
        if normalized.is_empty() {
            return Err(ParseVersionError::new(s, ParseVersionErrorKind::Empty));
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
                ParseVersionErrorKind::InvalidCharacters,
            ));
        }

        // Find epoch
        let (epoch, rest) = if let Some((epoch, rest)) = lowered.split_once('!') {
            let _: usize = epoch.parse().map_err(|e| {
                ParseVersionError::new(s, ParseVersionErrorKind::EpochMustBeInteger(e))
            })?;
            ([epoch], rest)
        } else {
            (["0"], lowered.as_str())
        };

        // Ensure the rest of the string no longer contains an epoch
        if rest.find('!').is_some() {
            return Err(ParseVersionError::new(
                s,
                ParseVersionErrorKind::DuplicateEpochSeparator,
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
                ParseVersionErrorKind::DuplicateLocalVersionSeparator,
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
        ) -> Result<VersionComponent, ParseVersionErrorKind> {
            let mut result = VersionComponent::default();
            for component in split_iter {
                let version_split_re = lazy_regex::regex!(r#"([0-9]+|[^0-9]+)"#);
                let mut numeral_or_alpha_split = version_split_re.find_iter(component).peekable();
                if numeral_or_alpha_split.peek().is_none() {
                    return Err(ParseVersionErrorKind::EmptyVersionComponent);
                }
                let range_start = result.components.len();
                for numeral_or_alpha in numeral_or_alpha_split {
                    let numeral_or_alpha = numeral_or_alpha.as_str();
                    let parsed: NumeralOrOther = match numeral_or_alpha {
                        num if num.chars().all(|c| c.is_ascii_digit()) => num
                            .parse::<usize>()
                            .map_err(ParseVersionErrorKind::InvalidNumeral)?
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
