use super::{Component, Version};
use crate::version::{EPOCH_MASK, LOCAL_VERSION_MASK, LOCAL_VERSION_OFFSET};
use nom::branch::alt;
use nom::bytes::complete::{tag_no_case, take_while};
use nom::character::complete::{alpha1, char, digit1, one_of};
use nom::combinator::{cut, eof, map, opt, value};
use nom::error::{ErrorKind, FromExternalError, ParseError};
use nom::sequence::{preceded, terminated};
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
use thiserror::Error;
use crate::version::segment::Segment;

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
            ParseVersionErrorKind::ExpectedComponent => write!(f, "expected a version component"),
            ParseVersionErrorKind::ExpectedSegmentSeparator => write!(f, "expected '.', '-', or '_'"),
            ParseVersionErrorKind::CannotMixAndMatchDashesAndUnderscores => write!(f, "cannot mix and match underscores and dashes"),
            ParseVersionErrorKind::Nom(_) => write!(f, "parse error"),
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
#[derive(Debug, Eq, PartialEq, Clone, Error)]
pub enum ParseVersionErrorKind {
    /// The string was empty
    #[error("empty string")]
    Empty,
    /// The string contained invalid characters
    #[error("invalid characters")]
    InvalidCharacters,
    /// The epoch was not an integer value
    #[error("epoch is not a number")]
    EpochMustBeInteger(ParseIntError),
    /// The string contained an invalid numeral
    #[error("invalid number")]
    InvalidNumeral(ParseIntError),
    /// The string contained multiple epoch separators
    #[error("string contains multiple epoch seperators ('!')")]
    DuplicateEpochSeparator,
    /// The string contained multiple local version separators
    #[error("string contains multiple version seperators ('+')")]
    DuplicateLocalVersionSeparator,
    /// The string contained an empty version component
    #[error("expected a version component e.g. `2` or `rc`")]
    EmptyVersionComponent,
    /// Too many segments.
    #[error("the version string contains too many version segments")]
    TooManySegments,
    /// Too many segments.
    #[error("there are too many components in a single segment")]
    TooManyComponentsInASegment,
    /// Expected a version component
    #[error("expected a version component e.g. `2` or `rc`")]
    ExpectedComponent,
    /// Expected a segment seperator
    #[error("expected a '.', '-', or '_'")]
    ExpectedSegmentSeparator,
    /// Cannot mix and match dashes and underscores
    #[error("cannot use both underscores and dashes as version segment seperators")]
    CannotMixAndMatchDashesAndUnderscores,
    /// Nom error
    #[error("{0:?}")]
    Nom(ErrorKind),
}

impl<'i> ParseError<&'i str> for ParseVersionErrorKind {
    fn from_error_kind(_: &'i str, kind: ErrorKind) -> Self {
        ParseVersionErrorKind::Nom(kind)
    }

    fn append(_: &'i str, _: ErrorKind, other: Self) -> Self {
        other
    }
}

impl<'i> FromExternalError<&'i str, ParseVersionErrorKind> for ParseVersionErrorKind {
    fn from_external_error(_: &'i str, _: ErrorKind, e: ParseVersionErrorKind) -> Self {
        e
    }
}

/// Parses the epoch part of a version. This is a number followed by `'!'` at the start of the
/// version string.
pub fn epoch_parser(input: &str) -> IResult<&str, u64, ParseVersionErrorKind> {
    let (rest, digits) = terminated(digit1, char('!'))(input)?;
    let epoch = digits
        .parse()
        .map_err(|err| ParseVersionErrorKind::EpochMustBeInteger(err))
        .map_err(nom::Err::Failure)?;
    Ok((rest, epoch))
}

/// Parses a numeral from the input, fails if the parsed digits cannot be represented by an `u64`.
fn numeral_parser<'i>(input: &'i str) -> IResult<&'i str, u64, ParseVersionErrorKind> {
    let (rest, digits) = digit1(input)?;
    match u64::from_str(digits) {
        Ok(numeral) => Ok((rest, numeral)),
        Err(e) => Err(nom::Err::Failure(ParseVersionErrorKind::InvalidNumeral(e))),
    }
}

/// Parses a single version [`Component`].
fn component_parser<'i>(input: &'i str) -> IResult<&'i str, Component, ParseVersionErrorKind> {
    alt((
        // Parse a numeral
        map(numeral_parser, Component::Numeral),
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
fn segment_parser<'i, 'c>(
    components: &'c mut SmallVec<[Component; 3]>,
) -> impl Parser<&'i str, u16, ParseVersionErrorKind> + 'c {
    move |input| {
        // Parse the first component of the segment
        let (mut rest, first_component) = match component_parser(input) {
            Ok(result) => result,
            Err(nom::Err::Error(_)) => {
                return Err(nom::Err::Error(ParseVersionErrorKind::ExpectedComponent))
            }
            Err(e) => return Err(e),
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
                    segment_length = match segment_length.checked_add(1) {
                        Some(length) => length,
                        None => {
                            return Err(nom::Err::Error(
                                ParseVersionErrorKind::TooManyComponentsInASegment,
                            ))
                        }
                    }
                }
                None => {
                    break Ok((remaining, segment_length.try_into().unwrap()));
                }
            }
            rest = remaining;
        }
    }
}

fn final_version_part_parser<'i>(
    components: &mut SmallVec<[Component; 3]>,
    segment_lengths: &mut SmallVec<[u16; 4]>,
    input: &'i str,
    dash_or_underscore: Option<char>,
) -> Result<Option<char>, nom::Err<ParseVersionErrorKind>> {
    let mut dash_or_underscore = dash_or_underscore;
    let first_segment_idx = segment_lengths.len();

    // Parse the first segment of the version. It must exists.
    let (mut input, first_segment_length) = segment_parser(components).parse(input)?;
    segment_lengths.push(first_segment_length);
    let result = loop {
        // Parse either eof or a version segment separator.
        let (rest, separator) = match alt((map(one_of("-._"), Some), value(None, eof)))(input) {
            Ok((_, None)) => break Ok(dash_or_underscore),
            Ok((rest, Some(separator))) => (rest, separator),
            Err(nom::Err::Error(_)) => {
                break Err(nom::Err::Error(
                    ParseVersionErrorKind::ExpectedSegmentSeparator,
                ))
            }
            Err(e) => return Err(e),
        };

        // Make sure dashes and underscores are not mixed.
        match (dash_or_underscore, separator) {
            (None, seperator) => dash_or_underscore = Some(seperator),
            (Some('-'), '_') | (Some('_'), '-') => {
                break Err(nom::Err::Error(
                    ParseVersionErrorKind::CannotMixAndMatchDashesAndUnderscores,
                ))
            }
            _ => {}
        }

        // Parse the next segment.
        let (rest, segment_length) = match segment_parser(components).parse(rest) {
            Ok(result) => result,
            Err(e) => break Err(e),
        };
        segment_lengths.push(segment_length);

        input = rest;
    };

    // If there was an error, revert the `segment_lengths` array.
    if result.is_err() {
        segment_lengths.drain(first_segment_idx..);
    }

    result
}

pub fn version_parser<'i>(input: &'i str) -> IResult<&'i str, Version, ParseVersionErrorKind> {
    let mut components = SmallVec::default();
    let mut segments = SmallVec::default();
    let mut flags = 0u8;

    // String must not be empty
    if input.is_empty() {
        return Err(nom::Err::Error(ParseVersionErrorKind::Empty));
    }

    // Parse an optional epoch.
    let (input, epoch) = opt(epoch_parser)(input)?;
    if let Some(epoch) = epoch {
        components.push(epoch.into());
        flags |= EPOCH_MASK;
    }

    // Scan the input to find the version segments.
    let (rest, common_part) = recognize_segments(input)?;
    let (rest, local_part) = opt(preceded(char('+'), cut(recognize_segments)))(rest)?;

    // Parse the common version part
    let dash_or_underscore =
        final_version_part_parser(&mut components, &mut segment_lengths, common_part, None)?;

    // Parse the local version part
    if let Some(local_part) = local_part {
        let first_local_segment_idx = segment_lengths.len();

        // Check if there are not too many segments.
        if first_local_segment_idx > (LOCAL_VERSION_MASK >> LOCAL_VERSION_OFFSET) as usize {
            // There are too many segments to be able to encode the local segment parts into the
            // special `flag` we store. The flags is 8 bits and the first bit is used to
            // indicate if there is an epoch or not. The remaining 7 bits are used to indicate
            // which segment is the first that belongs to the local version part. We can encode
            // at most 127 positions so if there are more segments in the common version part,
            // we cannot represent this version.
            return Err(nom::Err::Error(ParseVersionErrorKind::TooManySegments));
        }

        // Encode that the local version segment starts at the given index. The unwrap is safe
        // because we checked that it would fit above.
        flags |= (u8::try_from(first_local_segment_idx).unwrap()) << LOCAL_VERSION_OFFSET;

        // Parse the segments
        final_version_part_parser(
            &mut components,
            &mut segment_lengths,
            local_part,
            dash_or_underscore,
        )?;
    }

    return Ok((
        rest,
        Version {
            norm: None,
            flags,
            components,
            segments,
        },
    ));

    /// A helper function to crudely recognize version segments.
    fn recognize_segments<'i, E: ParseError<&'i str>>(
        input: &'i str,
    ) -> IResult<&'i str, &'i str, E> {
        take_while(|c: char| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')(input)
    }
}

pub fn final_version_parser(input: &str) -> Result<Version, ParseVersionErrorKind> {
    match version_parser(input) {
        Ok(("", version)) => Ok(version),
        Ok(_) => Err(ParseVersionErrorKind::ExpectedSegmentSeparator),
        Err(nom::Err::Failure(e) | nom::Err::Error(e)) => Err(e),
        Err(_) => unreachable!("not streaming, so no other error possible"),
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
        return final_version_parser(s).map_err(|kind| ParseVersionError::new(s, kind));

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
                segments.push(Segment::new(component_count));
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
            segments,
            components,
        })
    }
}

#[cfg(test)]
mod test {
    use crate::version::{parse::final_version_parser, Version};
    use serde::Serialize;
    use std::collections::BTreeMap;

    #[test]
    fn test_parse() {
        let versions = [
            "$",
            ".",
            "1!1.2a.3-rc1",
            "1+",
            "1+$",
            "1+.",
            "1+2",
            "1-2-3",
            "1-2-3_",
            "1-2_3",
            "1.0.1_",
            "1.0.1post.za",
            "1@2",
            "1_",
            "1_2_3",
            "1_2_3_",
            "1__",
            "1___",
        ];

        #[derive(Serialize)]
        #[serde(untagged)]
        enum VersionOrError {
            Version(Version),
            Error(String),
        }

        let mut index_map: BTreeMap<String, VersionOrError> = BTreeMap::default();
        for version in versions {
            let version_or_error = match final_version_parser(version) {
                Ok(version) => VersionOrError::Version(version),
                Err(e) => VersionOrError::Error(e.to_string()),
            };
            index_map.insert(version.to_owned(), version_or_error);
        }

        insta::assert_toml_snapshot!(index_map);
    }
}
