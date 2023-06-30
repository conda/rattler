use super::{Component, Version};
use crate::version::flags::Flags;
use crate::version::segment::Segment;
use crate::version::{ComponentVec, SegmentVec};
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
        .map_err(ParseVersionErrorKind::EpochMustBeInteger)
        .map_err(nom::Err::Failure)?;
    Ok((rest, epoch))
}

/// Parses a numeral from the input, fails if the parsed digits cannot be represented by an `u64`.
fn numeral_parser(input: &str) -> IResult<&str, u64, ParseVersionErrorKind> {
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
fn segment_parser<'i>(
    components: &mut ComponentVec,
) -> impl Parser<&'i str, Segment, ParseVersionErrorKind> + '_ {
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
        let mut component_count = 0u16;
        let has_implicit_default = !first_component.is_numeric();

        // Add the first component
        components.push(first_component);
        component_count += 1;

        // Loop until we can't find any more components
        loop {
            let (remaining, component) = match opt(component_parser)(rest) {
                Ok((i, o)) => (i, o),
                Err(e) => {
                    // Remove any components that we may have added.
                    components.drain(components.len() - (component_count as usize)..);
                    return Err(e);
                }
            };
            match component {
                Some(component) => {
                    components.push(component);
                    component_count = match component_count.checked_add(1) {
                        Some(length) => length,
                        None => {
                            return Err(nom::Err::Error(
                                ParseVersionErrorKind::TooManyComponentsInASegment,
                            ))
                        }
                    }
                }
                None => {
                    let segment = Segment::new(component_count)
                        .ok_or(nom::Err::Error(
                            ParseVersionErrorKind::TooManyComponentsInASegment,
                        ))?
                        .with_implicit_default(has_implicit_default);

                    break Ok((remaining, segment));
                }
            }
            rest = remaining;
        }
    }
}

fn final_version_part_parser(
    components: &mut ComponentVec,
    segments: &mut SegmentVec,
    input: &str,
    dash_or_underscore: Option<char>,
) -> Result<Option<char>, nom::Err<ParseVersionErrorKind>> {
    let mut dash_or_underscore = dash_or_underscore;
    let first_segment_idx = segments.len();

    // Parse the first segment of the version. It must exists.
    let (mut input, first_segment_length) = segment_parser(components).parse(input)?;
    segments.push(first_segment_length);
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
        let (rest, segment) = match segment_parser(components).parse(rest) {
            Ok(result) => result,
            Err(e) => break Err(e),
        };
        segments.push(
            segment
                .with_separator(Some(separator))
                .expect("unrecognized separator"),
        );

        input = rest;
    };

    // If there was an error, revert the `segment_lengths` array.
    if result.is_err() {
        segments.drain(first_segment_idx..);
    }

    result
}

pub fn version_parser(input: &str) -> IResult<&str, Version, ParseVersionErrorKind> {
    let mut components = SmallVec::default();
    let mut segments = SmallVec::default();
    let mut flags = Flags::default();

    // String must not be empty
    if input.is_empty() {
        return Err(nom::Err::Error(ParseVersionErrorKind::Empty));
    }

    // Parse an optional epoch.
    let (input, epoch) = opt(epoch_parser)(input)?;
    if let Some(epoch) = epoch {
        components.push(epoch.into());
        flags = flags.with_has_epoch(true);
    }

    // Scan the input to find the version segments.
    let (rest, common_part) = recognize_segments(input)?;
    let (rest, local_part) = opt(preceded(char('+'), cut(recognize_segments)))(rest)?;

    // Parse the common version part
    let dash_or_underscore =
        final_version_part_parser(&mut components, &mut segments, common_part, None)?;

    // Parse the local version part
    if let Some(local_part) = local_part {
        let first_local_segment_idx = segments.len();

        // Encode the local segment index into the flags.
        match u8::try_from(first_local_segment_idx)
            .ok()
            .and_then(|idx| flags.with_local_segment_index(idx))
        {
            None => {
                // There are too many segments to be able to encode the local segment parts into the
                // special `flag` we store. The flags is 8 bits and the first bit is used to
                // indicate if there is an epoch or not. The remaining 7 bits are used to indicate
                // which segment is the first that belongs to the local version part. We can encode
                // at most 127 positions so if there are more segments in the common version part,
                // we cannot represent this version.
                return Err(nom::Err::Error(ParseVersionErrorKind::TooManySegments));
            }
            Some(updated_flags) => {
                flags = updated_flags;
            }
        }

        // Parse the segments
        final_version_part_parser(
            &mut components,
            &mut segments,
            local_part,
            dash_or_underscore,
        )?;
    }

    return Ok((
        rest,
        Version {
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

impl FromStr for Version {
    type Err = ParseVersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        final_version_parser(s).map_err(|kind| ParseVersionError::new(s, kind))
    }
}

#[cfg(test)]
mod test {
    use super::{final_version_parser, Version};
    use serde::Serialize;
    use std::collections::BTreeMap;
    use std::fmt::{Display, Formatter};

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

        #[derive(Debug, Serialize)]
        #[serde(untagged)]
        enum VersionOrError {
            Version(Version),
            Error(String),
        }

        let mut index_map: BTreeMap<String, VersionOrError> = BTreeMap::default();
        for version_str in versions {
            let version_or_error = match final_version_parser(version_str) {
                Ok(version) => {
                    assert_eq!(version_str, version.to_string().as_str());
                    VersionOrError::Version(version)
                }
                Err(e) => VersionOrError::Error(e.to_string()),
            };
            index_map.insert(version_str.to_owned(), version_or_error);
        }

        insta::assert_debug_snapshot!(index_map);
    }

    struct DisplayAsDebug<T>(T);

    impl<T> Display for DisplayAsDebug<T>
    where
        for<'i> &'i T: std::fmt::Debug,
    {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            write!(f, "{:?}", &self.0)
        }
    }
}
