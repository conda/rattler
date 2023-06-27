use super::{Component, Version};
use crate::version::{EPOCH_MASK, LOCAL_VERSION_MASK, LOCAL_VERSION_OFFSET};
use nom::branch::alt;
use nom::bytes::complete::{tag_no_case, take_while1};
use nom::character::complete::{alpha1, char, one_of, u64 as parse_u64};
use nom::character::is_alphanumeric;
use nom::combinator::{cut, eof, map, opt, peek, value};
use nom::error::{context, ContextError, ErrorKind, FromExternalError, ParseError};
use nom::sequence::{pair, preceded, terminated};
use nom::{IResult, Parser};
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
            ParseVersionErrorKind::TooManySegments => write!(f, "too many segments"),
            ParseVersionErrorKind::TooManyComponentsInASegment => write!(f, "too many version components, a single version segment can at most contain {} components", (1<<16)-1),
        }
    }
}

impl Error for ParseVersionError {}

impl ParseVersionError {
    /// Create a new parse error
    pub fn new(text: impl Into<String>, kind: ParseVersionErrorKind) -> Self {
        Self {
            version: text.into(),
            kind,
        }
    }
}

/// The type of parse error that occurred when parsing a version string.
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum ParseVersionErrorKind {
    /// The string was empty
    Empty,
    /// The string contained invalid characters
    InvalidCharacters,
    /// The epoch was not an integer value
    EpochMustBeInteger(ParseIntError),
    /// The string contained an invalid numeral
    InvalidNumeral(ParseIntError),
    /// The string contained multiple epoch separators
    DuplicateEpochSeparator,
    /// The string contained multiple local version separators
    DuplicateLocalVersionSeparator,
    /// The string contained an empty version component
    EmptyVersionComponent,
    /// Too many segments.
    TooManySegments,
    /// Too many segments.
    TooManyComponentsInASegment,
}

/// Parses the epoch part of a version. This is a number followed by `'!'` at the start of the
/// version string.
pub fn epoch_parser<'i, E: ParseError<&'i str>>(input: &'i str) -> IResult<&'i str, u64, E> {
    terminated(parse_u64, char('!'))(input)
}

/// Parses a single version [`Component`].
fn component_parser<
    'i,
    E: ParseError<&'i str> + FromExternalError<&'i str, ParseVersionErrorKind>,
>(
    input: &'i str,
) -> IResult<&'i str, Component, E> {
    alt((
        // Parse a numeral
        map(parse_u64, Component::Numeral),
        // Parse special case components
        value(Component::Post, tag_no_case("post")),
        value(Component::Dev, tag_no_case("dev")),
        // Parse an identifier
        map(alpha1, |alpha: &'i str| {
            Component::Iden(alpha.to_lowercase().into_boxed_str())
        }),
        // Parse a `_` at the end of the string.
        map(terminated(char('_'), eof), |_| {
            Component::Iden(String::from("_").into_boxed_str())
        }),
    ))(input)
}

/// Parses a version segment from a list of components.
fn segment_parser<
    'i,
    'c,
    E: ParseError<&'i str> + FromExternalError<&'i str, ParseVersionErrorKind>,
>(
    components: &'c mut SmallVec<[Component; 3]>,
) -> impl Parser<&'i str, u16, E> + 'c {
    move |input| {
        // Parse the first component of the segment
        let (mut rest, first_component) = opt(component_parser)(input)?;
        let first_component = match first_component {
            Some(first_component) => first_component,
            None => {
                return Err(nom::Err::Error(E::from_external_error(
                    input,
                    ErrorKind::Fail,
                    ParseVersionErrorKind::EmptyVersionComponent,
                )))
            }
        };

        // If the first component is not numeric we add a default component since each segment must
        // always start with a number.
        let mut segment_length = 0;
        if !first_component.is_numeric() {
            components.push(Component::default());
            segment_length += 1;
        }

        // Add the first component
        components.push(first_component);
        segment_length += 1;

        // Loop until we can't find any more components
        loop {
            let (remaining, component) = match opt(component_parser)(rest) {
                Ok((i, o)) => (i, o),
                Err(e) => {
                    // Remove any components that we may have added.
                    components.drain(components.len() - segment_length..);
                    return Err(e);
                }
            };
            match component {
                Some(component) => {
                    components.push(component);
                    segment_length += 1;
                }
                None => {
                    break Ok((remaining, segment_length.try_into().unwrap()));
                }
            }
            rest = remaining;
        }
    }
}

pub fn version_parser<
    'i,
    E: ParseError<&'i str> + ContextError<&'i str> + FromExternalError<&'i str, ParseVersionErrorKind>,
>(
    input: &'i str,
) -> IResult<&'i str, Version, E> {
    let mut components = SmallVec::default();
    let mut segment_lengths = SmallVec::default();
    let mut flags = 0u8;

    // Parse an optional epoch.
    let (input, epoch) = context("epoch", opt(epoch_parser))(input)?;
    if let Some(epoch) = epoch {
        components.push(epoch.into());
        flags |= EPOCH_MASK;
    }

    // Scan the input to find the version segments.
    let (rest, mut common_part) = recognize_segments(input)?;
    let (rest, local_part) = opt(preceded(
        char('+'),
        context("local", cut(recognize_segments)),
    ))(rest)?;

    // Parse the first segment of the version. It must exists.
    let (mut common_part, first_segment_length) =
        context("segment", segment_parser(&mut components))(common_part)?;
    segment_lengths.push(first_segment_length);

    // Parse the rest of the segments
    let mut dash_or_underscore = None;
    let common_part_remaining = loop {
        // Parse a separator and another segment
        let (rest, opt_segment) =
            opt(pair(one_of(".-_"), cut(segment_parser(&mut components))))(common_part)?;
        let (separator, segment_length) = match opt_segment {
            Some(next_segment) => next_segment,
            None => break rest,
        };

        // Make sure dashes and underscores are not mixed.
        match (dash_or_underscore, separator) {
            (None, '-') => dash_or_underscore = Some('-'),
            (None, '_') => dash_or_underscore = Some('_'),
            (Some('-'), '_') | (Some('-'), '-') => {
                return Err(nom::Err::Error(E::from_external_error(
                    &common_part[..1],
                    ErrorKind::Fail,
                    ParseVersionErrorKind::InvalidCharacters,
                )))
            }
            _ => {}
        }

        segment_lengths.push(segment_length);
        common_part = rest;
    };

    return Ok((
        rest,
        Version {
            norm: None,
            flags,
            components,
            segment_lengths,
        },
    ));

    /// A helper function to crudely recognize version segments.
    fn recognize_segments<'i, E: ParseError<&'i str>>(
        input: &'i str,
    ) -> IResult<&'i str, &'i str, E> {
        take_while1(|c: char| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')(input)
    }
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
            let epoch: u64 = epoch.parse().map_err(|e| {
                ParseVersionError::new(s, ParseVersionErrorKind::EpochMustBeInteger(e))
            })?;
            (Some(epoch), rest)
        } else {
            (None, lowered.as_str())
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
        let version_split = version.iter().map(|s| s.as_str());

        let mut components = SmallVec::default();
        let mut segments = SmallVec::default();
        let mut flags = 0u8;

        if let Some(epoch) = epoch {
            components.push(epoch.into());
            flags |= 0x1; // Mark that the version contains an epoch
        }

        fn split_component<'a>(
            segments_iter: impl Iterator<Item = &'a str>,
            segments: &mut SmallVec<[u16; 4]>,
            components: &mut SmallVec<[Component; 3]>,
        ) -> Result<(), ParseVersionErrorKind> {
            for component in segments_iter {
                let version_split_re = lazy_regex::regex!(r#"([0-9]+|[^0-9]+)"#);
                let mut numeral_or_alpha_split = version_split_re.find_iter(component).peekable();
                if numeral_or_alpha_split.peek().is_none() {
                    return Err(ParseVersionErrorKind::EmptyVersionComponent);
                }

                let mut atoms = numeral_or_alpha_split
                    .map(|mtch| match mtch.as_str() {
                        num if num.chars().all(|c| c.is_ascii_digit()) => num
                            .parse()
                            .map_err(ParseVersionErrorKind::InvalidNumeral)
                            .map(Component::Numeral),
                        "post" => Ok(Component::Post),
                        "dev" => Ok(Component::Dev),
                        ident => Ok(Component::Iden(ident.to_owned().into_boxed_str())),
                    })
                    .peekable();

                // A segment must always starts with a numeral
                let mut component_count = 0u16;
                if !matches!(atoms.peek(), Some(&Ok(Component::Numeral(_)))) {
                    components.push(Component::Numeral(0));
                    component_count = component_count
                        .checked_add(1)
                        .ok_or(ParseVersionErrorKind::TooManyComponentsInASegment)?;
                }

                // Add the components
                for component in atoms {
                    components.push(component?);
                    component_count = component_count
                        .checked_add(1)
                        .ok_or(ParseVersionErrorKind::TooManyComponentsInASegment)?;
                }

                // Add the segment information
                segments.push(component_count);
            }

            Ok(())
        }

        split_component(version_split, &mut segments, &mut components)
            .map_err(|e| ParseVersionError::new(s, e))?;

        if !local.is_empty() {
            if segments.len() > (LOCAL_VERSION_MASK >> LOCAL_VERSION_OFFSET) as usize {
                // There are too many segments to be able to encode the local segment parts into the
                // special `flag` we store. The flags is 8 bits and the first bit is used to
                // indicate if there is an epoch or not. The remaining 7 bits are used to indicate
                // which segment is the first that belongs to the local version part. We can encode
                // at most 127 positions so if there are more segments in the common version part,
                // we cannot represent this version.
                return Err(ParseVersionError::new(
                    s,
                    ParseVersionErrorKind::TooManySegments,
                ));
            }

            // Encode that the local version segment starts at the given index.
            flags |= (u8::try_from(segments.len()).unwrap()) << LOCAL_VERSION_OFFSET;

            split_component(local_split, &mut segments, &mut components)
                .map_err(|e| ParseVersionError::new(s, e))?
        };

        Ok(Self {
            norm: Some(lowered.into_boxed_str()),
            flags,
            segment_lengths: segments,
            components,
        })
    }
}

#[cfg(test)]
mod test {
    use crate::version::{parse::version_parser, Version};
    use nom::combinator::all_consuming;
    use serde::Serialize;
    use std::collections::BTreeMap;

    #[test]
    fn test_parse() {
        let versions = ["1!1.2a.3-rc1", "1_", "1__", "1___"];

        #[derive(Debug)]
        enum VersionOrError {
            Version(Version),
            Error(String),
        }

        let mut index_map: BTreeMap<String, VersionOrError> = BTreeMap::default();
        for version in versions {
            let version_or_error = match all_consuming(version_parser)(version) {
                Ok((_, version)) => VersionOrError::Version(version),
                Err(nom::Err::Error(e)) | Err(nom::Err::Failure(e)) => {
                    VersionOrError::Error(nom::error::convert_error(version, e))
                }
                _ => unreachable!(),
            };
            index_map.insert(version.to_owned(), version_or_error);
        }

        insta::assert_debug_snapshot!(index_map);
    }
}
