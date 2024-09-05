use super::{Component, StrictVersion, Version};
use crate::version::flags::Flags;
use crate::version::segment::Segment;
use crate::version::{ComponentVec, SegmentVec};
use nom::branch::alt;
use nom::bytes::complete::tag_no_case;
use nom::character::complete::{alpha1, char, digit1, one_of};
use nom::combinator::{map, opt, value};
use nom::error::{ErrorKind, FromExternalError, ParseError};
use nom::sequence::terminated;
use nom::IResult;
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
        write!(
            f,
            "malformed version string '{}': {}",
            &self.version, &self.kind
        )
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
    /// The epoch was not an integer value
    #[error("epoch is not a number")]
    EpochMustBeInteger(ParseIntError),
    /// The string contained an invalid numeral
    #[error("invalid number")]
    InvalidNumeral(ParseIntError),
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
    /// Expected a segment separator
    #[error("expected a '.', '-', or '_'")]
    ExpectedSegmentSeparator,
    /// Cannot mix and match dashes and underscores
    #[error("cannot use both underscores and dashes as version segment separators")]
    CannotMixAndMatchDashesAndUnderscores,
    /// Expected the end of the string
    #[error("encountered more characters but expected none")]
    ExpectedEof,
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
    ))(input)
}

/// Parses a version segment from a list of components.
fn segment_parser<'i>(
    components: &mut ComponentVec,
    input: &'i str,
) -> IResult<&'i str, Segment, ParseVersionErrorKind> {
    // Parse the first component of the segment
    let (mut rest, first_component) = match component_parser(input) {
        Ok(result) => result,
        // Convert undefined parse errors into an expect error
        Err(nom::Err::Error(ParseVersionErrorKind::Nom(_))) => {
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
        if let Some(component) = component {
            components.push(component);
            component_count = match component_count.checked_add(1) {
                Some(length) => length,
                None => {
                    return Err(nom::Err::Failure(
                        ParseVersionErrorKind::TooManyComponentsInASegment,
                    ))
                }
            }
        } else {
            let segment = Segment::new(component_count)
                .ok_or(nom::Err::Failure(
                    ParseVersionErrorKind::TooManyComponentsInASegment,
                ))?
                .with_implicit_default(has_implicit_default);

            break Ok((remaining, segment));
        }
        rest = remaining;
    }
}

/// Parses a trailing underscore or dash.
fn trailing_dash_underscore_parser(
    input: &str,
    dash_or_underscore: Option<char>,
) -> IResult<&str, (Option<Component>, Option<char>), ParseVersionErrorKind> {
    // Parse a - or _. Return early if it cannot be found.
    let (rest, Some(separator)) = opt(one_of::<_, _, (&str, ErrorKind)>("-_"))(input)
        .map_err(|e| e.map(|(_, kind)| ParseVersionErrorKind::Nom(kind)))?
    else {
        return Ok((input, (None, dash_or_underscore)));
    };

    // Make sure dashes and underscores are not mixed.
    let dash_or_underscore = match (dash_or_underscore, separator) {
        (None, '-') => Some('-'),
        (None, '_') => Some('_'),
        (Some('-'), '_') | (Some('_'), '-') => {
            return Err(nom::Err::Error(
                ParseVersionErrorKind::CannotMixAndMatchDashesAndUnderscores,
            ))
        }
        _ => dash_or_underscore,
    };

    Ok((
        rest,
        (
            Some(Component::UnderscoreOrDash {
                is_dash: separator == '-',
            }),
            dash_or_underscore,
        ),
    ))
}

fn version_part_parser<'i>(
    components: &mut ComponentVec,
    segments: &mut SegmentVec,
    input: &'i str,
    dash_or_underscore: Option<char>,
) -> IResult<&'i str, Option<char>, ParseVersionErrorKind> {
    let mut dash_or_underscore = dash_or_underscore;
    let mut recovery_segment_idx = segments.len();

    // Parse the first segment of the version. It must exists.
    let (mut input, first_segment_length) = segment_parser(components, input)?;
    segments.push(first_segment_length);

    // Iterate over any additional segments that we find.
    let result = loop {
        // Parse a version segment separator.
        let (rest, separator) = match opt(one_of("-._"))(input) {
            Ok((_, None)) => {
                // No additional separator found, exit early.
                return Ok((input, dash_or_underscore));
            }
            Ok((rest, Some(separator))) => (rest, separator),

            Err(nom::Err::Error(_)) => {
                // If an error occurred we convert it to a segment separator not found error instead.
                break Err(nom::Err::Error(
                    ParseVersionErrorKind::ExpectedSegmentSeparator,
                ));
            }

            // Failure are propagated
            Err(e) => break Err(e),
        };

        // Make sure dashes and underscores are not mixed.
        match (dash_or_underscore, separator) {
            (None, '-') => dash_or_underscore = Some('-'),
            (None, '_') => dash_or_underscore = Some('_'),
            (Some('-'), '_') | (Some('_'), '-') => {
                break Err(nom::Err::Failure(
                    ParseVersionErrorKind::CannotMixAndMatchDashesAndUnderscores,
                ))
            }
            _ => {}
        }

        // Parse the next segment.
        let (rest, segment) = match segment_parser(components, rest) {
            Ok(result) => result,
            Err(nom::Err::Error(_)) => {
                // If parsing of a segment failed, check if perhaps the separator is followed by an
                // underscore or dash.
                match trailing_dash_underscore_parser(rest, dash_or_underscore)? {
                    (rest, (Some(component), dash_or_underscore)) => {
                        // We are parsing multiple dashes or underscores ("..__"), add a new segment
                        // just for the trailing underscore/dash
                        components.push(component);
                        segments.push(
                            Segment::new(1)
                                .unwrap()
                                .with_implicit_default(true)
                                .with_separator(Some(separator))
                                .unwrap(),
                        );

                        // Since the trailing is always at the end we immediately return
                        return Ok((rest, dash_or_underscore));
                    }
                    (rest, (None, dash_or_underscore)) if separator == '-' || separator == '_' => {
                        // We are parsing a single dash or underscore (".._"), update the last
                        // segment we added
                        let segment = segments
                            .last_mut()
                            .expect("there must be at least one segment added");
                        components.push(Component::UnderscoreOrDash {
                            is_dash: separator == '-',
                        });

                        *segment = segment
                            .len()
                            .checked_add(1)
                            .and_then(|len| segment.with_component_count(len))
                            .ok_or(nom::Err::Failure(
                                ParseVersionErrorKind::TooManyComponentsInASegment,
                            ))?;

                        // Since the trailing is always at the end we immediately return
                        return Ok((rest, dash_or_underscore));
                    }
                    _ => return Ok((input, dash_or_underscore)),
                }
            }

            // Failures are propagated
            Err(e) => break Err(e),
        };

        segments.push(
            segment
                .with_separator(Some(separator))
                .expect("unrecognized separator"),
        );
        recovery_segment_idx += 1;
        input = rest;
    };

    match result {
        // If there was an error, revert the `segment_lengths` array.
        Err(e) => {
            segments.drain(recovery_segment_idx..);
            Err(e)
        }
        Ok(separator) => Ok((input, separator)),
    }
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

    // Parse the common part of the version
    let (rest, dash_or_underscore) =
        match version_part_parser(&mut components, &mut segments, input, None) {
            Ok(result) => result,
            Err(e) => return Err(e),
        };

    // Parse the local version part
    let rest = if let Ok((local_version_part, _)) = char::<_, (&str, ErrorKind)>('+')(rest) {
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

        match version_part_parser(
            &mut components,
            &mut segments,
            local_version_part,
            dash_or_underscore,
        ) {
            Ok((rest, _)) => rest,
            Err(e) => return Err(e),
        }
    } else {
        rest
    };

    Ok((
        rest,
        Version {
            components,
            segments,
            flags,
        },
    ))
}

impl FromStr for Version {
    type Err = ParseVersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match version_parser(s) {
            Ok(("", version)) => Ok(version),
            Ok(_) => Err(ParseVersionError::new(
                s,
                ParseVersionErrorKind::ExpectedEof,
            )),
            Err(nom::Err::Failure(e) | nom::Err::Error(e)) => Err(ParseVersionError::new(s, e)),
            Err(_) => unreachable!("not streaming, so no other error possible"),
        }
    }
}

impl FromStr for StrictVersion {
    type Err = ParseVersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(StrictVersion(Version::from_str(s)?))
    }
}

#[cfg(test)]
mod test {
    use super::Version;
    use crate::version::parse::version_parser;
    use crate::version::SegmentFormatter;
    use serde::Serialize;
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::str::FromStr;

    #[test]
    fn test_parse_star() {
        assert_eq!(
            version_parser("1.*"),
            Ok((".*", Version::from_str("1").unwrap()))
        );
    }

    #[test]
    fn test_parse() {
        #[derive(Debug, Serialize)]
        #[serde(untagged)]
        enum VersionOrError {
            Version(Version),
            Error(String),
        }

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
            "1.0.1-",
            "1.0.1post.za",
            "1@2",
            "1_",
            "1_2_3",
            "1_2_3_",
            "1__",
            "1___",
            "1--",
            "1-_",
            "1_-",
        ];

        let mut index_map: BTreeMap<String, VersionOrError> = BTreeMap::default();
        for version_str in versions {
            let version_or_error = match Version::from_str(version_str) {
                Ok(version) => {
                    assert_eq!(version_str, version.to_string().as_str());
                    VersionOrError::Version(version)
                }
                Err(e) => VersionOrError::Error(e.kind.to_string()),
            };
            index_map.insert(version_str.to_owned(), version_or_error);
        }

        insta::assert_debug_snapshot!(index_map);
    }

    /// Parse a large number of versions and see if parsing succeeded.
    /// TODO: This doesnt really verify that the parsing is correct. Maybe we can parse the version
    /// with Conda too and verify that the results match?
    #[test]
    fn test_parse_all() {
        let versions = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data/parsed_versions.txt"),
        )
        .unwrap();
        for line in versions.lines() {
            // Skip comments and empty lines
            if line.trim_start().starts_with('#') || line.trim().is_empty() {
                continue;
            }

            let (version, debug_parsed) = line.split_once('=').unwrap();
            let parsed_version = Version::from_str(version).unwrap();
            let parsed_version_debug_string = format!(
                "{:?}",
                SegmentFormatter::new(
                    Some(parsed_version.epoch_opt().unwrap_or(0)),
                    parsed_version.segments()
                )
            );
            assert_eq!(parsed_version_debug_string, debug_parsed);
        }
    }
}
