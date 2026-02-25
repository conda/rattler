use std::{borrow::Cow, collections::HashSet, ops::Not, str::FromStr, sync::Arc};

use nom::{
    branch::alt,
    bytes::complete::{tag, take_till1, take_until, take_while, take_while1},
    character::complete::{char, multispace0, multispace1, one_of, space0},
    combinator::{opt, recognize},
    error::{context, ContextError, ParseError},
    multi::{separated_list0, separated_list1},
    sequence::{delimited, preceded, separated_pair, terminated},
    Finish, IResult, Parser,
};
use rattler_digest::{parse_digest_from_hex, Md5, Sha256};
use smallvec::SmallVec;
use thiserror::Error;
use typed_path::Utf8TypedPath;
use url::Url;

use super::{
    matcher::{StringMatcher, StringMatcherParseError},
    MatchSpec,
};
use crate::match_spec::condition::parse_condition;
use crate::{
    build_spec::{BuildNumberSpec, ParseBuildNumberSpecError},
    match_spec::package_name_matcher::{PackageNameMatcher, PackageNameMatcherParseError},
    package::CondaArchiveIdentifier,
    utils::{path::is_absolute_path, url::parse_scheme},
    version_spec::{
        is_start_of_version_constraint,
        version_tree::{recognize_constraint, recognize_version},
        ParseVersionSpecError,
    },
    Channel, ChannelConfig, NamelessMatchSpec, ParseChannelError, ParseMatchSpecOptions,
    ParseStrictness, ParseVersionError, Platform, VersionSpec,
};

/// The type of parse error that occurred when parsing match spec.
#[derive(Debug, Clone, Error, PartialEq)]
pub enum ParseMatchSpecError {
    /// The path or url of the package was invalid
    #[error("invalid package path or url")]
    InvalidPackagePathOrUrl,

    /// Invalid package spec url
    #[error("invalid package spec url")]
    InvalidPackageUrl(#[from] url::ParseError),

    /// Invalid version in path or url
    #[error(transparent)]
    InvalidPackagePathOrUrlVersion(#[from] ParseVersionError),

    /// Invalid bracket in match spec
    #[error("invalid bracket")]
    InvalidBracket,

    /// Invalid channel provided in match spec
    #[error("invalid channel")]
    ParseChannelError(#[from] ParseChannelError),

    /// Invalid key in match spec
    #[error("invalid bracket key: {0}")]
    InvalidBracketKey(String),

    /// Missing package name in match spec
    #[error("missing package name")]
    MissingPackageName,

    /// Multiple bracket sections in match spec
    #[error("multiple bracket sections not allowed")]
    MultipleBracketSectionsNotAllowed,

    /// Invalid version and build
    #[error("unable to parse version spec: {0}")]
    InvalidVersionAndBuild(String),

    /// Invalid build string
    #[error(
        "the build string '{0}' is not valid, it can only contain alphanumeric characters and underscores"
    )]
    InvalidBuildString(String),

    /// Invalid version spec
    #[error(transparent)]
    InvalidVersionSpec(#[from] ParseVersionSpecError),

    /// Invalid string matcher
    #[error(transparent)]
    InvalidStringMatcher(#[from] StringMatcherParseError),

    /// Invalid build number spec
    #[error("invalid build number spec: {0}")]
    InvalidBuildNumber(#[from] ParseBuildNumberSpecError),

    /// Unable to parse hash digest from hex
    #[error("unable to parse hash digest from hex")]
    InvalidHashDigest,

    /// The package name matcher was invalid
    #[error(transparent)]
    InvalidPackageNameMatcher(#[from] PackageNameMatcherParseError),

    /// Multiple values for a key in the matchspec
    #[error("found multiple values for: {0}")]
    MultipleValueForKey(String),

    /// More than one semicolon in match spec
    #[error("more than one semicolon in match spec")]
    MoreThanOneSemicolon,

    /// Deprecated `; if` syntax used
    #[error("the '; if' syntax for conditional dependencies is deprecated, use '[when=\"...\"]' bracket syntax instead")]
    DeprecatedIfSyntax,

    /// Invalid condition in match spec
    #[error("could not parse condition {0}: {1}")]
    InvalidCondition(String, String),

    /// Only exact package name matchers are allowed but a glob was provided
    #[error("\"{0}\" looks like a glob but only exact package names are allowed, package names can only contain 0-9, a-z, A-Z, -, _, or .")]
    OnlyExactPackageNameMatchersAllowedGlob(String),

    /// Only exact package name matchers are allowed but a regex was provided
    #[error("\"{0}\" looks like a regex but only exact package names are allowed, package names can only contain 0-9, a-z, A-Z, -, _, or .")]
    OnlyExactPackageNameMatchersAllowedRegex(String),
}

impl FromStr for MatchSpec {
    type Err = ParseMatchSpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str(s, ParseMatchSpecOptions::default())
    }
}

impl MatchSpec {
    /// Parses a [`MatchSpec`] from a string with a given strictness.
    pub fn from_str(
        source: &str,
        options: impl Into<ParseMatchSpecOptions>,
    ) -> Result<Self, ParseMatchSpecError> {
        matchspec_parser(source, options.into())
    }
}

/// Strips a comment from a match spec. A comment is preceded by a '#' followed
/// by the comment itself. This functions splits the matchspec into the
/// matchspec and comment part.
fn strip_comment(input: &str) -> (&str, Option<&str>) {
    input
        .split_once('#')
        .map_or_else(|| (input, None), |(spec, comment)| (spec, Some(comment)))
}

/// Rejects the deprecated `; if` syntax and returns an error if found.
/// Users should migrate to the new `[when="..."]` bracket syntax.
/// Also returns an error for bare semicolons (more than one).
fn reject_deprecated_if_syntax(input: &str) -> Result<&str, ParseMatchSpecError> {
    // Fast path: no semicolons at all (99%+ of match specs)
    if input.find(';').is_none() {
        return Ok(input.trim());
    }

    // Check for deprecated "; if" syntax first (more helpful error)
    if has_if_statement(input) {
        return Err(ParseMatchSpecError::DeprecatedIfSyntax);
    }

    // Check that we only have a single semicolon (if any)
    if input.matches(';').count() > 1 {
        return Err(ParseMatchSpecError::MoreThanOneSemicolon);
    }

    // No deprecated syntax found, return the input as is
    Ok(input.trim())
}

/// Check if the input contains the deprecated `; if` syntax
fn has_if_statement(input: &str) -> bool {
    let mut parser = (
        // Take everything up to "; if"
        nom::bytes::complete::take_until::<_, _, nom::error::Error<&str>>(";"),
        // Match "; if " with flexible whitespace
        (
            multispace0::<_, nom::error::Error<&str>>,
            char(';'),
            multispace0,
            tag("if"),
            multispace1, // At least one whitespace after "if"
        ),
    );

    parser.parse(input).is_ok()
}

/// An optimized data structure to store key value pairs in between a bracket
/// string `[key1=value1, key2=value2]`. The optimization stores two such values
/// on the stack and otherwise allocates a vector on the heap. Two is chosen
/// because that seems to be more than enough for most use cases.
type BracketVec<'a> = SmallVec<[(&'a str, &'a str); 2]>;

/// A parse combinator to filter whitespace if front and after another parser.
fn whitespace_enclosed<'a, F, O, E: ParseError<&'a str>>(
    mut inner: F,
) -> impl Parser<&'a str, Output = O, Error = E>
where
    F: Parser<&'a str, Output = O, Error = E>,
{
    move |input: &'a str| {
        let (input, _) = multispace0(input)?;
        let (input, o2) = inner.parse(input)?;
        multispace0(input).map(|(i, _)| (i, o2))
    }
}

/// Parses a quoted string that may contain escape sequences.
/// Returns the content between quotes (with escape sequences still in place).
/// Supports both single and double quotes, and handles escaped quotes within.
fn parse_quoted_string_with_escapes<'a>(
    quote_char: char,
) -> impl FnMut(&'a str) -> IResult<&'a str, &'a str> {
    move |input: &'a str| {
        // Match opening quote
        let (input, _) = char(quote_char)(input)?;

        // Fast path: no escape sequences, use simple search
        if !input.contains('\\') {
            if let Some(pos) = input.find(quote_char) {
                return Ok((&input[pos + quote_char.len_utf8()..], &input[..pos]));
            }
            return Err(nom::Err::Error(nom::error::Error::new(
                input,
                nom::error::ErrorKind::Char,
            )));
        }

        // Slow path: handle escape sequences
        let mut chars = input.char_indices();
        let mut end_pos = None;

        while let Some((i, c)) = chars.next() {
            if c == '\\' {
                // Skip the next character (it's escaped)
                chars.next();
            } else if c == quote_char {
                end_pos = Some(i);
                break;
            }
        }

        match end_pos {
            Some(pos) => {
                let content = &input[..pos];
                let remaining = &input[pos + quote_char.len_utf8()..];
                Ok((remaining, content))
            }
            None => Err(nom::Err::Error(nom::error::Error::new(
                input,
                nom::error::ErrorKind::Char,
            ))),
        }
    }
}

/// Escapes a string value for use in bracket syntax.
/// Escapes double quotes and backslashes.
/// Returns `Cow::Borrowed` when no escaping is needed (the common case).
///
/// This is the inverse of [`unescape_string`].
pub(crate) fn escape_bracket_value(s: &str) -> Cow<'_, str> {
    if !s.contains('"') && !s.contains('\\') {
        return Cow::Borrowed(s);
    }
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            _ => result.push(c),
        }
    }
    Cow::Owned(result)
}

/// Unescapes a string by processing escape sequences.
/// Handles \", \', and \\ escape sequences.
///
/// This is the inverse of [`escape_bracket_value`].
pub(crate) fn unescape_string(input: &str) -> Cow<'_, str> {
    if !input.contains('\\') {
        return Cow::Borrowed(input);
    }

    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                match next {
                    '"' | '\'' | '\\' => {
                        result.push(next);
                        chars.next();
                    }
                    _ => {
                        // Keep the backslash for unrecognized escapes
                        result.push(c);
                    }
                }
            } else {
                result.push(c);
            }
        } else {
            result.push(c);
        }
    }

    Cow::Owned(result)
}

/// Parses the contents of a bracket list `[version="1,2,3", bla=3]`
fn parse_bracket_list(input: &str) -> Result<BracketVec<'_>, ParseMatchSpecError> {
    /// Parses a key in a bracket string
    fn parse_key(input: &str) -> IResult<&str, &str> {
        whitespace_enclosed(context(
            "key",
            take_while(|c: char| c.is_alphanumeric() || c == '_' || c == '-'),
        ))
        .parse(input)
    }

    /// Parses a value in a bracket string.
    fn parse_value(input: &str) -> IResult<&str, &str> {
        whitespace_enclosed(context(
            "value",
            alt((
                parse_quoted_string_with_escapes('"'),
                parse_quoted_string_with_escapes('\''),
                delimited(char('['), take_until("]"), char(']')),
                take_till1(|c| c == ',' || c == ']' || c == '\'' || c == '"'),
            )),
        ))
        .parse(input)
    }

    /// Parses a `key=value` pair
    fn parse_key_value(input: &str) -> IResult<&str, (&str, &str)> {
        separated_pair(parse_key, char('='), parse_value).parse(input)
    }

    /// Parses a list of `key=value` pairs separated by commas
    fn parse_key_value_list(input: &str) -> IResult<&str, Vec<(&str, &str)>> {
        separated_list0(whitespace_enclosed(char(',')), parse_key_value).parse(input)
    }

    /// Parses an entire bracket string
    fn parse_bracket_list(input: &str) -> IResult<&str, Vec<(&str, &str)>> {
        delimited(char('['), parse_key_value_list, char(']')).parse(input)
    }

    match parse_bracket_list(input).finish() {
        Ok((_remaining, values)) => Ok(values.into()),
        Err(nom::error::Error { .. }) => Err(ParseMatchSpecError::InvalidBracket),
    }
}

/// Strips the brackets part of the matchspec returning the rest of the
/// matchspec and  the contents of the brackets as a `Vec<&str>`.
fn strip_brackets(input: &str) -> Result<(Cow<'_, str>, BracketVec<'_>), ParseMatchSpecError> {
    // Fast path: skip the regex entirely if no brackets present.
    if !input.contains('[') {
        return Ok((input.into(), SmallVec::new()));
    }

    if let Some(matches) =
        lazy_regex::regex!(r#".*(\[(?:[^\[\]]|\[(?:[^\[\]]|\[.*\])*\])*\])$"#).captures(input)
    {
        let bracket_str = matches.get(1).unwrap().as_str();
        let bracket_contents = parse_bracket_list(bracket_str)?;

        let input = if let Some(input) = input.strip_suffix(bracket_str) {
            Cow::Borrowed(input)
        } else {
            Cow::Owned(input.replace(bracket_str, ""))
        };

        Ok((input, bracket_contents))
    } else {
        Ok((input.into(), SmallVec::new()))
    }
}

/// Parses a list of optional dependencies from a string `feat1, feat2, feat3]`
/// -> `vec![feat1, feat2, feat3]`.
pub fn parse_extras(input: &str) -> Result<Vec<String>, ParseMatchSpecError> {
    use nom::{
        combinator::{all_consuming, map},
        multi::separated_list1,
    };

    fn parse_feature_name(i: &str) -> IResult<&str, &str> {
        delimited(
            multispace0,
            take_while1(|c: char| c.is_alphanumeric() || c == '_' || c == '-'),
            multispace0,
        )
        .parse(i)
    }

    fn parse_features(i: &str) -> IResult<&str, Vec<String>> {
        separated_list1(char(','), map(parse_feature_name, |s: &str| s.to_string())).parse(i)
    }

    match all_consuming(parse_features).parse(input).finish() {
        Ok((_remaining, features)) => Ok(features),
        Err(_e) => Err(ParseMatchSpecError::InvalidBracket),
    }
}

/// Parses a [`BracketVec`] into precise components
fn parse_bracket_vec_into_components(
    bracket: BracketVec<'_>,
    match_spec: NamelessMatchSpec,
    options: ParseMatchSpecOptions,
) -> Result<NamelessMatchSpec, ParseMatchSpecError> {
    let mut match_spec = match_spec;

    if options.strictness() == ParseStrictness::Strict {
        // check for duplicate keys
        let mut seen = HashSet::new();
        for (key, _) in &bracket {
            if seen.contains(key) {
                return Err(ParseMatchSpecError::MultipleValueForKey((*key).to_string()));
            }
            seen.insert(key);
        }
    }

    for elem in bracket {
        let (key, value) = elem;
        match key {
            "version" => {
                match_spec.version = Some(VersionSpec::from_str(value, options.strictness())?);
            }
            "build" => match_spec.build = Some(StringMatcher::from_str(value)?),
            "build_number" => match_spec.build_number = Some(BuildNumberSpec::from_str(value)?),
            "extras" => {
                // Optional features are still experimental
                if options.allow_experimental_extras() {
                    match_spec.extras = Some(parse_extras(value)?);
                } else {
                    return Err(ParseMatchSpecError::InvalidBracketKey("extras".to_string()));
                }
            }
            "sha256" => {
                match_spec.sha256 = Some(
                    parse_digest_from_hex::<Sha256>(value)
                        .ok_or(ParseMatchSpecError::InvalidHashDigest)?,
                );
            }
            "md5" => {
                match_spec.md5 = Some(
                    parse_digest_from_hex::<Md5>(value)
                        .ok_or(ParseMatchSpecError::InvalidHashDigest)?,
                );
            }
            "fn" => match_spec.file_name = Some(value.to_string()),
            "url" => {
                // Is the spec an url, parse it as an url
                let url = if parse_scheme(value).is_some() {
                    Url::parse(value)?
                }
                // 2 Is the spec an absolute path, parse it as an url
                else if is_absolute_path(value) {
                    let path = Utf8TypedPath::from(value);
                    file_url::file_path_to_url(path)
                        .map_err(|_error| ParseMatchSpecError::InvalidPackagePathOrUrl)?
                } else {
                    return Err(ParseMatchSpecError::InvalidPackagePathOrUrl);
                };

                match_spec.url = Some(url);
            }
            "subdir" => match_spec.subdir = Some(value.to_string()),
            "channel" => {
                let (channel, subdir) = parse_channel_and_subdir(value)?;
                match_spec.channel = match_spec.channel.or(channel.map(Arc::new));
                match_spec.subdir = match_spec.subdir.or(subdir);
            }
            "license" => match_spec.license = Some(value.to_string()),
            "track_features" => {
                match_spec.track_features = Some(
                    value
                        .split([',', ' ']) // Split on BOTH comma and space
                        .map(str::trim) // Remove surrounding whitespace
                        .filter(|s| !s.is_empty()) // Filter out empty strings from "a, b"
                        .map(ToString::to_string)
                        .collect(),
                );
            }
            "when" => {
                // Conditional dependencies using bracket syntax
                if options.allow_experimental_conditionals() {
                    // Unescape the value in case it contains escaped quotes
                    let unescaped_value = unescape_string(value);
                    let (remainder, condition) =
                        parse_condition(&unescaped_value).map_err(|e| {
                            ParseMatchSpecError::InvalidCondition(value.to_string(), e.to_string())
                        })?;

                    if !remainder.trim().is_empty() {
                        return Err(ParseMatchSpecError::InvalidCondition(
                            value.to_string(),
                            "remainder not empty".to_string(),
                        ));
                    }

                    match_spec.condition = Some(condition);
                } else {
                    return Err(ParseMatchSpecError::InvalidBracketKey("when".to_string()));
                }
            }
            // TODO: Still need to add `features` and `license_family`
            // to the match spec.
            _ => Err(ParseMatchSpecError::InvalidBracketKey(key.to_owned()))?,
        }
    }

    Ok(match_spec)
}

/// Parses an url or path like string into an url.
pub fn parse_url_like(input: &str) -> Result<Option<Url>, ParseMatchSpecError> {
    // Skip if channel is provided, this avoids parsing namespaces as urls
    if input.contains("::") {
        return Ok(None);
    }

    // Is the spec an url, parse it as an url
    if parse_scheme(input).is_some() {
        return Url::parse(input)
            .map(Some)
            .map_err(ParseMatchSpecError::from);
    }
    // Is the spec a path, parse it as an url
    if is_absolute_path(input) {
        let path = Utf8TypedPath::from(input);
        return file_url::file_path_to_url(path)
            .map(Some)
            .map_err(|_err| ParseMatchSpecError::InvalidPackagePathOrUrl);
    }
    Ok(None)
}

/// Strip the package name from the input.
fn strip_package_name(
    input: &str,
    exact_names_only: bool,
) -> Result<(PackageNameMatcher, &str), ParseMatchSpecError> {
    let (rest, package_name) =
        take_while1(|c: char| !c.is_whitespace() && !is_start_of_version_constraint(c))(
            input.trim(),
        )
        .finish()
        .map_err(|_err: nom::error::Error<_>| ParseMatchSpecError::MissingPackageName)?;

    let trimmed_package_name = package_name.trim();
    if trimmed_package_name.is_empty() {
        return Err(ParseMatchSpecError::MissingPackageName);
    }

    let rest = rest.trim();

    let package_name = match PackageNameMatcher::from_str(trimmed_package_name)
        .map_err(ParseMatchSpecError::InvalidPackageNameMatcher)?
    {
        PackageNameMatcher::Exact(name) => PackageNameMatcher::Exact(name),
        PackageNameMatcher::Glob(glob) => {
            if exact_names_only {
                return Err(
                    ParseMatchSpecError::OnlyExactPackageNameMatchersAllowedGlob(
                        glob.as_str().to_string(),
                    ),
                );
            } else {
                PackageNameMatcher::Glob(glob)
            }
        }
        PackageNameMatcher::Regex(regex) => {
            if exact_names_only {
                return Err(
                    ParseMatchSpecError::OnlyExactPackageNameMatchersAllowedRegex(
                        regex.as_str().to_string(),
                    ),
                );
            } else {
                PackageNameMatcher::Regex(regex)
            }
        }
    };

    Ok((package_name, rest))
}

/// Splits a string into version and build constraints.
fn split_version_and_build(
    input: &str,
    strictness: ParseStrictness,
) -> Result<(&str, Option<&str>), ParseMatchSpecError> {
    fn maybe_recognize_lenient_constraint<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
        strictness: ParseStrictness,
    ) -> impl FnMut(&'a str) -> IResult<&'a str, &'a str, E> {
        move |input: &'a str| {
            if strictness == ParseStrictness::Lenient {
                alt((parse_special_equality, recognize_constraint)).parse(input)
            } else {
                recognize_constraint(input)
            }
        }
    }

    fn parse_version_constraint_or_group<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
        strictness: ParseStrictness,
    ) -> impl FnMut(&'a str) -> IResult<&'a str, &'a str, E> {
        move |input: &'a str| {
            alt((
                delimited(tag("("), parse_version_group(strictness), tag(")")),
                maybe_recognize_lenient_constraint(strictness),
            ))
            .parse(input)
        }
    }

    fn parse_version_group<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
        strictness: ParseStrictness,
    ) -> impl FnMut(&'a str) -> IResult<&'a str, &'a str, E> {
        move |input: &'a str| {
            recognize(separated_list1(
                whitespace_enclosed(one_of(",|")),
                parse_version_constraint_or_group(strictness),
            ))
            .parse(input)
        }
    }

    // Special case handling of `=*`, `=1.2.3`, or `=1*`
    fn parse_special_equality<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
        input: &'a str,
    ) -> IResult<&'a str, &'a str, E> {
        // Matches ".*" or "*" but not "."
        let version_glob = terminated(opt(tag(".")), tag("*"));

        // Matches a version followed by an optional version glob
        let version_followed_by_glob = terminated(recognize_version(true), opt(version_glob));

        // Just matches the glob operator ("*")
        let just_star = tag("*");

        recognize(preceded(
            tag("="),
            alt((version_followed_by_glob, just_star)),
        ))
        .parse(input)
    }

    fn parse_version_and_build_separator<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
        strictness: ParseStrictness,
    ) -> impl FnMut(&'a str) -> IResult<&'a str, &'a str, E> {
        move |input: &'a str| {
            if strictness == ParseStrictness::Lenient {
                terminated(parse_version_group(strictness), opt(one_of(" ="))).parse(input)
            } else {
                terminated(parse_version_group(strictness), space0).parse(input)
            }
        }
    }

    match parse_version_and_build_separator::<nom::error::Error<&str>>(strictness)(input).finish() {
        Ok((rest, version)) => {
            let build_string = rest.trim();

            // Check validity of the build string
            if strictness == ParseStrictness::Strict
                && build_string.contains(|c: char| !c.is_alphanumeric() && c != '_' && c != '*')
            {
                return Err(ParseMatchSpecError::InvalidBuildString(
                    build_string.to_owned(),
                ));
            }

            Ok((
                version.trim(),
                build_string.is_empty().not().then_some(build_string),
            ))
        }
        Err(_) => Err(ParseMatchSpecError::InvalidVersionAndBuild(
            input.to_string(),
        )),
    }
}
/// Parse version and build string.
fn parse_version_and_build(
    input: &str,
    strictness: ParseStrictness,
) -> Result<(Option<VersionSpec>, Option<StringMatcher>), ParseMatchSpecError> {
    if input.find('[').is_some() {
        return Err(ParseMatchSpecError::MultipleBracketSectionsNotAllowed);
    }

    let (version_str, build_str) = split_version_and_build(input, strictness)?;

    let version_str = if version_str.find(char::is_whitespace).is_some() {
        Cow::Owned(version_str.replace(char::is_whitespace, ""))
    } else {
        Cow::Borrowed(version_str)
    };

    // Under certain circumstances we strip the `=` or `==` parts of the version
    // string. See the function for more info.
    let version_str = optionally_strip_equals(&version_str, build_str, strictness);

    // Parse the version spec
    let version = Some(
        VersionSpec::from_str(version_str.as_ref(), strictness)
            .map_err(ParseMatchSpecError::InvalidVersionSpec)?,
    );

    // Parse the build string
    let mut build = None;
    if let Some(build_str) = build_str {
        build = Some(StringMatcher::from_str(build_str)?);
    }

    Ok((version, build))
}

impl FromStr for NamelessMatchSpec {
    type Err = ParseMatchSpecError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::from_str(input, ParseMatchSpecOptions::default())
    }
}

impl NamelessMatchSpec {
    /// Parses a [`NamelessMatchSpec`] from a string with a given strictness.
    pub fn from_str(
        input: &str,
        options: impl Into<ParseMatchSpecOptions>,
    ) -> Result<Self, ParseMatchSpecError> {
        let options = options.into();

        let input = input.trim();

        // Check for deprecated "; if" syntax
        let input = reject_deprecated_if_syntax(input)?;

        // Strip off brackets portion
        let (input, brackets) = strip_brackets(input)?;
        let input = input.trim();

        // Parse url or path spec
        if let Some(url) = parse_url_like(input)? {
            return Ok(NamelessMatchSpec {
                url: Some(url),
                ..NamelessMatchSpec::default()
            });
        }

        let mut match_spec =
            parse_bracket_vec_into_components(brackets, NamelessMatchSpec::default(), options)?;

        // 5. Strip of ':' to find channel and namespace
        // This assumes the [*] portions is stripped off, and then strip reverse to
        // ignore the first colon As that might be in the channel url.
        let mut input_split = input.rsplitn(3, ':').fuse();
        let input = input_split.next().unwrap_or("");
        let namespace = input_split.next();
        let channel_str = input_split.next();

        match_spec.namespace = namespace
            .map(str::trim)
            .filter(|namespace| !namespace.is_empty())
            .map(ToOwned::to_owned)
            .or(match_spec.namespace);

        if let Some(channel_str) = channel_str {
            let (channel, subdir) = parse_channel_and_subdir(channel_str)?;
            match_spec.channel = match_spec.channel.or(channel.map(Arc::new));
            match_spec.subdir = match_spec.subdir.or(subdir);
        }

        // Get the version and optional build string
        if !input.is_empty() {
            let (version, build) = parse_version_and_build(input, options.strictness())?;
            if options.strictness() == ParseStrictness::Strict {
                if match_spec.version.is_some() && version.is_some() {
                    return Err(ParseMatchSpecError::MultipleValueForKey(
                        "version".to_owned(),
                    ));
                }

                if match_spec.build.is_some() && build.is_some() {
                    return Err(ParseMatchSpecError::MultipleValueForKey("build".to_owned()));
                }
            }
            match_spec.version = match_spec.version.or(version);
            match_spec.build = match_spec.build.or(build);
        }

        Ok(match_spec)
    }
}

/// Parse channel and subdir from a string.
fn parse_channel_and_subdir(
    input: &str,
) -> Result<(Option<Channel>, Option<String>), ParseMatchSpecError> {
    let channel_config = ChannelConfig::default_with_root_dir(
        std::env::current_dir().expect("Could not get current directory"),
    );

    if let Some((channel, subdir)) = input.rsplit_once('/') {
        // If the subdir is a platform, we assume the channel has a subdir
        if Platform::from_str(subdir).is_ok() {
            return Ok((
                Some(Channel::from_str(channel, &channel_config)?),
                Some(subdir.to_string()),
            ));
        }
    }
    Ok((Some(Channel::from_str(input, &channel_config)?), None))
}

/// Parses a conda match spec.
/// This is based on: <https://github.com/conda/conda/blob/master/conda/models/match_spec.py#L569>
pub(crate) fn matchspec_parser(
    input: &str,
    options: ParseMatchSpecOptions,
) -> Result<MatchSpec, ParseMatchSpecError> {
    // Step 1. Strip '#' comment
    let (input, _comment) = strip_comment(input);

    // Check for deprecated "; if" syntax and return error if found
    // (Users should migrate to the new [when="..."] bracket syntax)
    let input = reject_deprecated_if_syntax(input)?;

    // 2. Strip off brackets portion
    let (input, brackets) = strip_brackets(input.trim())?;
    let mut nameless_match_spec =
        parse_bracket_vec_into_components(brackets, NamelessMatchSpec::default(), options)?;

    // 3. Strip off parens portion
    // TODO: What is this? I've never seen it

    // 4. Parse as url
    if nameless_match_spec.url.is_none() {
        // 4. Parse as url
        if let Some(url) = parse_url_like(&input)? {
            let archive = CondaArchiveIdentifier::try_from_url(&url);
            let name = archive.and_then(|a| PackageNameMatcher::from_str(&a.identifier.name).ok());

            if let Some(name) = name {
                return Ok(MatchSpec::from_nameless(
                    NamelessMatchSpec {
                        url: Some(url),
                        ..Default::default()
                    },
                    name,
                ));
            } else {
                // If we can't figure out the name from the URL, return an error
                return Err(ParseMatchSpecError::MissingPackageName);
            }
        }
    }

    // 5. Strip of ':' to find channel and namespace
    // This assumes the [*] portions is stripped off, and then strip reverse to
    // ignore the first colon As that might be in the channel url.
    let mut input_split = input.rsplitn(3, ':').fuse();
    let input = input_split.next().unwrap_or("");
    let namespace = input_split.next();
    let channel_str = input_split.next();

    nameless_match_spec.namespace = namespace
        .map(str::trim)
        .filter(|namespace| !namespace.is_empty())
        .map(ToOwned::to_owned)
        .or(nameless_match_spec.namespace);

    if let Some(channel_str) = channel_str {
        let (channel, subdir) = parse_channel_and_subdir(channel_str)?;
        nameless_match_spec.channel = nameless_match_spec.channel.or(channel.map(Arc::new));
        nameless_match_spec.subdir = nameless_match_spec.subdir.or(subdir);
    }

    // Step 6. Strip off the package name from the input
    let (name, input) = strip_package_name(input, options.exact_names_only())?;
    let mut match_spec = MatchSpec::from_nameless(nameless_match_spec, name);

    // Step 7. Otherwise, sort our version + build
    let input = input.trim();
    if !input.is_empty() {
        let (version, build) = parse_version_and_build(input, options.strictness())?;
        if options.strictness() == ParseStrictness::Strict {
            if match_spec.version.is_some() && version.is_some() {
                return Err(ParseMatchSpecError::MultipleValueForKey(
                    "version".to_owned(),
                ));
            }

            if match_spec.build.is_some() && build.is_some() {
                return Err(ParseMatchSpecError::MultipleValueForKey("build".to_owned()));
            }
        }
        match_spec.version = match_spec.version.or(version);
        match_spec.build = match_spec.build.or(build);
    }

    Ok(match_spec)
}

/// HERE BE DRAGONS!
///
/// In some circumstances we strip the `=` or `==` parts of the version string.
/// This is for conda legacy reasons. This function implements that behavior and
/// returns the stripped/updated version.
///
/// Most of this is only done in lenient mode. In strict mode we don't do any of
/// this.
fn optionally_strip_equals<'a>(
    version_str: &'a str,
    build_str: Option<&str>,
    strictness: ParseStrictness,
) -> Cow<'a, str> {
    // If the version doesn't start with `=` then don't strip anything.
    let Some(version_without_equals) = version_str.strip_prefix('=') else {
        return version_str.into();
    };

    // If we are not in lenient mode then stop processing at this point. Any other
    // special case parsing behavior is only part of lenient mode parsing.
    if strictness != ParseStrictness::Lenient {
        return version_str.into();
    }

    // In lenient mode we have special case handling of version strings that start
    // with `==`. If the version starts with `==` and the build string is none
    // we strip the `==` part.
    //
    // This results in versions like `==1.0.*.*` being parsed as `1.0.*`.
    // `==1.0.*.*` is considered a regex pattern and therefor not supported, but
    // `1.0.*.*` is parsed as `1.0.*` in lenient mode. This is all very
    // confusing and weird and is therefor only enabled in lenient parsing mode.
    if let (Some(version_without_equals_equals), None) =
        (version_without_equals.strip_prefix('='), build_str)
    {
        return version_without_equals_equals.into();
    }

    // Check if this version is part of a grouping or not. E.g. `2|>3|=4`
    let is_grouping = version_without_equals.contains(['=', ',', '|']);
    if is_grouping {
        // If the version string forms a group we leave it untouched.
        return version_str.into();
    }

    // If the version is not part of a grouping and doesn't end in a `*` and there
    // is no build string then we add a `*`.
    if build_str.is_none() && !version_without_equals.ends_with('*') {
        format!("{version_without_equals}*").into()
    } else {
        // Otherwise if it is not part of a group we simply remove the `=` without
        // adding the `*`.
        version_without_equals.into()
    }
}

#[cfg(test)]
mod tests {
    use std::{str::FromStr, sync::Arc};

    use assert_matches::assert_matches;
    use indexmap::IndexMap;
    use rattler_digest::{parse_digest_from_hex, Md5, Sha256};
    use rstest::rstest;
    use serde::Serialize;
    use smallvec::smallvec;
    use url::Url;

    use super::{
        parse_channel_and_subdir, split_version_and_build, strip_brackets, strip_package_name,
        unescape_string, BracketVec, MatchSpec, ParseMatchSpecError,
    };
    use crate::match_spec::parse::parse_extras;
    use crate::{
        match_spec::parse::parse_bracket_list, BuildNumberSpec, Channel, ChannelConfig,
        NamelessMatchSpec, ParseChannelError, ParseMatchSpecOptions, ParseStrictness,
        ParseStrictness::*, Version, VersionSpec,
    };

    fn channel_config() -> ChannelConfig {
        ChannelConfig::default_with_root_dir(
            std::env::current_dir().expect("Could not get current directory"),
        )
    }

    #[test]
    fn test_strip_brackets() {
        let result = strip_brackets(r#"bla [version="1.2.3"]"#).unwrap();
        assert_eq!(result.0, "bla ");
        let expected: BracketVec<'_> = smallvec![("version", "1.2.3")];
        assert_eq!(result.1, expected);

        let result = strip_brackets(r#"bla [version='1.2.3']"#).unwrap();
        assert_eq!(result.0, "bla ");
        let expected: BracketVec<'_> = smallvec![("version", "1.2.3")];
        assert_eq!(result.1, expected);

        let result = strip_brackets(r#"bla [version=1]"#).unwrap();
        assert_eq!(result.0, "bla ");
        let expected: BracketVec<'_> = smallvec![("version", "1")];
        assert_eq!(result.1, expected);

        let result = strip_brackets(r#"conda-forge::bla[version=1]"#).unwrap();
        assert_eq!(result.0, "conda-forge::bla");
        let expected: BracketVec<'_> = smallvec![("version", "1")];
        assert_eq!(result.1, expected);

        let result = strip_brackets(r#"bla [version="1.2.3", build_number=1]"#).unwrap();
        assert_eq!(result.0, "bla ");
        let expected: BracketVec<'_> = smallvec![("version", "1.2.3"), ("build_number", "1")];
        assert_eq!(result.1, expected);
    }

    #[test]
    fn test_split_version_and_build() {
        assert_matches!(
            split_version_and_build("2.7|>=3.6", Lenient),
            Ok(("2.7|>=3.6", None))
        );

        assert_matches!(
            split_version_and_build("==1.0=py27_0", Lenient),
            Ok(("==1.0", Some("py27_0")))
        );
        assert_matches!(
            split_version_and_build("=*=cuda", Lenient),
            Ok(("=*", Some("cuda")))
        );
        assert_matches!(
            split_version_and_build("=1.2.3 0", Lenient),
            Ok(("=1.2.3", Some("0")))
        );
        assert_matches!(
            split_version_and_build("1.2.3=0", Lenient),
            Ok(("1.2.3", Some("0")))
        );
        assert_matches!(
            split_version_and_build(">=1.0 , < 2.0 py34_0", Lenient),
            Ok((">=1.0 , < 2.0", Some("py34_0")))
        );
        assert_matches!(
            split_version_and_build(">=1.0 , < 2.0 =py34_0", Lenient),
            Ok((">=1.0 , < 2.0", Some("=py34_0")))
        );
        assert_matches!(
            split_version_and_build("=1.2.3 ", Lenient),
            Ok(("=1.2.3", None))
        );
        assert_matches!(
            split_version_and_build(">1.8,<2|==1.7", Lenient),
            Ok((">1.8,<2|==1.7", None))
        );
        assert_matches!(
            split_version_and_build("* openblas_0", Lenient),
            Ok(("*", Some("openblas_0")))
        );
        assert_matches!(
            split_version_and_build("* *", Lenient),
            Ok(("*", Some("*")))
        );
        assert_matches!(
            split_version_and_build(">=1!164.3095,<1!165", Lenient),
            Ok((">=1!164.3095,<1!165", None))
        );

        assert_matches!(
            split_version_and_build("==1!164.3095,<1!165=py27_0", Strict),
            Err(ParseMatchSpecError::InvalidBuildString(_))
        );

        assert_matches!(
            split_version_and_build("3.8.* *_cpython", Lenient),
            Ok(("3.8.*", Some("*_cpython")))
        );
        assert_matches!(
            split_version_and_build("3.8.* *_cpython", Strict),
            Ok(("3.8.*", Some("*_cpython")))
        );
    }

    #[test]
    fn test_nameless_match_spec() {
        insta::assert_yaml_snapshot!([
            NamelessMatchSpec::from_str("3.8.* *_cpython", Strict).unwrap(),
            NamelessMatchSpec::from_str("==1.0 py27_0[fn=\"bla\"]", Strict).unwrap(),
            NamelessMatchSpec::from_str("=1.0 py27_0", Strict).unwrap(),
            NamelessMatchSpec::from_str("*cpu*", Strict).unwrap(),
            // the next two tests are a bit weird, the version is `foobar` and the channel is `conda-forge`
            NamelessMatchSpec::from_str("conda-forge::==foobar", Strict).unwrap(),
            NamelessMatchSpec::from_str("==foobar[channel=conda-forge]", Strict).unwrap(),
            NamelessMatchSpec::from_str("* [build=foo]", Strict).unwrap(),
            NamelessMatchSpec::from_str(">=1.2[build=foo]", Strict).unwrap(),
            NamelessMatchSpec::from_str("[version='>=1.2', build=foo]", Strict).unwrap(),
        ],
        @r###"
        - version: 3.8.*
          build: "*_cpython"
        - version: "==1.0"
          build: py27_0
          file_name: bla
        - version: 1.0.*
          build: py27_0
        - version: "*"
          build: cpu*
        - version: "==foobar"
          channel:
            base_url: "https://conda.anaconda.org/conda-forge"
            name: conda-forge
        - version: "==foobar"
          channel:
            base_url: "https://conda.anaconda.org/conda-forge"
            name: conda-forge
        - version: "*"
          build: foo
        - version: ">=1.2"
          build: foo
        - version: ">=1.2"
          build: foo
        "###);
    }

    #[test]
    fn test_match_spec_more() {
        let spec = MatchSpec::from_str("conda-forge::foo[version=\"1.0.*\"]", Strict).unwrap();
        assert_eq!(spec.name, "foo".parse().unwrap());
        assert_eq!(
            spec.version,
            Some(VersionSpec::from_str("1.0.*", Strict).unwrap())
        );
        assert_eq!(
            spec.channel,
            Some(
                Channel::from_str("conda-forge", &channel_config())
                    .map(Arc::new)
                    .unwrap()
            )
        );

        let spec = MatchSpec::from_str("conda-forge::foo[version=1.0.*]", Strict).unwrap();
        assert_eq!(spec.name, "foo".parse().unwrap());
        assert_eq!(
            spec.version,
            Some(VersionSpec::from_str("1.0.*", Strict).unwrap())
        );
        assert_eq!(
            spec.channel,
            Some(
                Channel::from_str("conda-forge", &channel_config())
                    .map(Arc::new)
                    .unwrap()
            )
        );

        let spec = MatchSpec::from_str(
            r#"conda-forge::foo[version=1.0.*, build_number=">6"]"#,
            Strict,
        )
        .unwrap();
        assert_eq!(spec.name, "foo".parse().unwrap());
        assert_eq!(
            spec.version,
            Some(VersionSpec::from_str("1.0.*", Strict).unwrap())
        );
        assert_eq!(
            spec.channel,
            Some(
                Channel::from_str("conda-forge", &channel_config())
                    .map(Arc::new)
                    .unwrap()
            )
        );
        assert_eq!(
            spec.build_number,
            Some(BuildNumberSpec::from_str(">6").unwrap())
        );
    }

    #[test]
    fn test_nameless_url() {
        let url_str =
            "https://conda.anaconda.org/conda-forge/linux-64/py-rattler-0.6.1-py39h8169da8_0.conda";
        let url = Url::parse(url_str).unwrap();
        let spec1 = NamelessMatchSpec::from_str(url_str, Strict).unwrap();
        assert_eq!(spec1.url, Some(url.clone()));

        let spec_with_brackets =
            NamelessMatchSpec::from_str(format!("[url={url_str}]").as_str(), Strict).unwrap();
        assert_eq!(spec_with_brackets.url, Some(url));
    }

    #[test]
    fn test_nameless_url_path() {
        // Windows
        let win_path_str = "C:\\Users\\user\\conda-bld\\linux-64\\foo-1.0-py27_0.tar.bz2";
        let spec = NamelessMatchSpec::from_str(win_path_str, Strict).unwrap();
        let win_path = file_url::file_path_to_url(win_path_str).unwrap();
        assert_eq!(spec.url, Some(win_path.clone()));

        let spec_with_brackets =
            NamelessMatchSpec::from_str(format!("[url={win_path_str}]").as_str(), Strict).unwrap();
        assert_eq!(spec_with_brackets.url, Some(win_path));

        // Unix
        let unix_path_str = "/users/user/conda-bld/linux-64/foo-1.0-py27_0.tar.bz2";
        let spec = NamelessMatchSpec::from_str(unix_path_str, Strict).unwrap();
        let unix_path = file_url::file_path_to_url(unix_path_str).unwrap();
        assert_eq!(spec.url, Some(unix_path.clone()));

        let spec_with_brackets =
            NamelessMatchSpec::from_str(format!("[url={unix_path_str}]").as_str(), Strict).unwrap();
        assert_eq!(spec_with_brackets.url, Some(unix_path));
    }

    #[test]
    fn test_hash_spec() {
        let spec = MatchSpec::from_str("conda-forge::foo[md5=1234567890]", Strict);
        assert_matches!(spec, Err(ParseMatchSpecError::InvalidHashDigest));

        let spec = MatchSpec::from_str("conda-forge::foo[sha256=1234567890]", Strict);
        assert_matches!(spec, Err(ParseMatchSpecError::InvalidHashDigest));

        let spec = MatchSpec::from_str("conda-forge::foo[sha256=315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3]", Strict).unwrap();
        assert_eq!(
            spec.sha256,
            Some(
                parse_digest_from_hex::<Sha256>(
                    "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3"
                )
                .unwrap()
            )
        );

        let spec = MatchSpec::from_str(
            "conda-forge::foo[md5=8b1a9953c4611296a827abf8c47804d7]",
            Strict,
        )
        .unwrap();
        assert_eq!(
            spec.md5,
            Some(parse_digest_from_hex::<Md5>("8b1a9953c4611296a827abf8c47804d7").unwrap())
        );
    }

    #[test]
    fn test_parse_bracket_list() {
        assert_eq!(
            parse_bracket_list("[version=1.0.1]").unwrap().as_ref(),
            &[("version", "1.0.1")]
        );
        assert_eq!(
            parse_bracket_list("[version='1.0.1']").unwrap().as_ref(),
            &[("version", "1.0.1")]
        );
        assert_eq!(
            parse_bracket_list("[version=\"1.0.1\"]").unwrap().as_ref(),
            &[("version", "1.0.1")]
        );

        assert_eq!(
            parse_bracket_list("[version=1.0.1, build=3]")
                .unwrap()
                .as_ref(),
            &[("version", "1.0.1"), ("build", "3")]
        );
        assert_eq!(
            parse_bracket_list("[version='1.0.1', build=3]")
                .unwrap()
                .as_ref(),
            &[("version", "1.0.1"), ("build", "3")]
        );
        assert_eq!(
            parse_bracket_list("[version=\"1.0.1\", build=3]")
                .unwrap()
                .as_ref(),
            &[("version", "1.0.1"), ("build", "3")]
        );

        assert_eq!(
            parse_bracket_list("[build=\"py2*\"]").unwrap().as_ref(),
            &[("build", "py2*")]
        );

        assert_eq!(
            parse_bracket_list("[build=py2*]").unwrap().as_ref(),
            &[("build", "py2*")]
        );

        assert_eq!(
            parse_bracket_list("[version=\"1.3,2.0\"]")
                .unwrap()
                .as_ref(),
            &[("version", "1.3,2.0")]
        );
    }

    #[rstest]
    #[case::lenient(Lenient)]
    #[case::strict(Strict)]
    fn test_from_str(#[case] strictness: ParseStrictness) {
        #[derive(Serialize)]
        #[serde(untagged)]
        enum MatchSpecOrError {
            Error { error: String },
            MatchSpec(Box<MatchSpec>),
        }

        // A list of matchspecs to parse.
        // Please keep this list sorted.
        let specs = [
            "blas *.* mkl",
            "C:\\Users\\user\\conda-bld\\linux-64\\foo-1.0-py27_0.tar.bz2",
            "foo=1.0=py27_0",
            "foo==1.0=py27_0",
            "https://conda.anaconda.org/conda-forge/linux-64/py-rattler-0.6.1-py39h8169da8_0.conda",
            "https://repo.prefix.dev/ruben-arts/linux-64/boost-cpp-1.78.0-h75c5d50_1.tar.bz2",
            "python 3.8.* *_cpython",
            "pytorch=*=cuda*",
            "x264 >=1!164.3095,<1!165",
            "/home/user/conda-bld/linux-64/foo-1.0-py27_0.tar.bz2",
            "conda-forge::foo[version=1.0.*]",
            "conda-forge::foo[version=1.0.*, build_number=\">6\"]",
            "python ==2.7.*.*|>=3.6",
            "python=3.9",
            "python=*",
            "https://software.repos.intel.com/python/conda::python[version=3.9]",
            "https://c.com/p/conda/linux-64::python[version=3.9]",
            "https://c.com/p/conda::python[version=3.9, subdir=linux-64]",
            // subdir in brackets take precedence
            "conda-forge/linux-32::python[version=3.9, subdir=linux-64]",
            "conda-forge/linux-32::python ==3.9[subdir=linux-64, build_number=\"0\"]",
            "python ==3.9[channel=conda-forge]",
            "python ==3.9[channel=conda-forge/linux-64]",
            "rust ~=1.2.3",
            "~/channel/dir::package",
            "~\\windows_channel::package",
            "./relative/channel::package",
            "python[channel=https://conda.anaconda.org/python/conda,version=3.9]",
            "channel/win-64::foobar[channel=conda-forge, subdir=linux-64]",
            // Issue #1004
            "numpy>=2.*.*",
            // Pixi issue 3922
            "bird_tool_utils_python =0.*,>=0.4.1",
        ];

        let evaluated: IndexMap<_, _> = specs
            .iter()
            .map(|spec| {
                (
                    spec,
                    MatchSpec::from_str(spec, strictness).map_or_else(
                        |err| MatchSpecOrError::Error {
                            error: err.to_string(),
                        },
                        |err| MatchSpecOrError::MatchSpec(Box::new(err)),
                    ),
                )
            })
            .collect();

        // Strip absolute paths to this crate from the channels for testing
        let crate_root = env!("CARGO_MANIFEST_DIR");
        let crate_path = Url::from_directory_path(std::path::Path::new(crate_root)).unwrap();

        let home_str = dirs::home_dir()
            .and_then(|p| Url::from_directory_path(p).ok())
            .map_or_else(|| "file:///dummy_home/".to_string(), |u| u.to_string());

        insta::with_settings!({filters => vec![
            (crate_path.as_str(), "file://<CRATE>/"),
            (home_str.as_str(), "file://<HOME>/"),
        ]}, {
            insta::assert_yaml_snapshot!(
                format!("test_from_string_{strictness:?}"),
                evaluated
            );
        });
    }

    #[rstest]
    #[case::lenient(Lenient)]
    #[case::strict(Strict)]
    fn test_nameless_from_str(#[case] strictness: ParseStrictness) {
        #[derive(Serialize)]
        #[serde(untagged)]
        enum NamelessSpecOrError {
            Error { error: String },
            Spec(Box<NamelessMatchSpec>),
        }

        // A list of matchspecs to parse.
        // Please keep this list sorted.
        let specs = [
            "2.7|>=3.6",
            "https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2",
            "~=1.2.3",
            "*.* mkl",
            "C:\\Users\\user\\conda-bld\\linux-64\\foo-1.0-py27_0.tar.bz2",
            "=1.0=py27_0",
            "==1.0=py27_0",
            "https://conda.anaconda.org/conda-forge/linux-64/py-rattler-0.6.1-py39h8169da8_0.conda",
            "https://repo.prefix.dev/ruben-arts/linux-64/boost-cpp-1.78.0-h75c5d50_1.tar.bz2",
            "3.8.* *_cpython",
            "=*=cuda*",
            ">=1!164.3095,<1!165",
            "/home/user/conda-bld/linux-64/foo-1.0-py27_0.tar.bz2",
            "[version=1.0.*]",
            "[version=1.0.*, build_number=\">6\"]",
            "==2.7.*.*|>=3.6",
            "3.9",
            "*",
            "[version=3.9]",
            "[version=3.9]",
            "[version=3.9, subdir=linux-64]",
            // subdir in brackets take precedence
            "[version=3.9, subdir=linux-64]",
            "==3.9[subdir=linux-64, build_number=\"0\"]",
            // Issue #1004
            ">=2.*.*",
        ];

        let evaluated: IndexMap<_, _> = specs
            .iter()
            .map(|spec| {
                (
                    spec,
                    NamelessMatchSpec::from_str(spec, strictness).map_or_else(
                        |err| NamelessSpecOrError::Error {
                            error: err.to_string(),
                        },
                        |err| NamelessSpecOrError::Spec(Box::new(err)),
                    ),
                )
            })
            .collect();
        insta::assert_yaml_snapshot!(
            format!("test_nameless_from_string_{strictness:?}"),
            evaluated
        );
    }

    #[test]
    fn test_invalid_bracket() {
        assert_matches!(
            strip_brackets(r#"bla [version="1.2.3", build_number=]"#),
            Err(ParseMatchSpecError::InvalidBracket)
        );
        assert_matches!(
            strip_brackets(r#"bla [version="1.2.3, build_number=1]"#),
            Err(ParseMatchSpecError::InvalidBracket)
        );
    }

    #[test]
    fn test_invalid_bracket_key() {
        let _unknown_key = String::from("unknown");
        let spec = MatchSpec::from_str("conda-forge::foo[unknown=1.0.*]", Strict);
        assert_matches!(
            spec,
            Err(ParseMatchSpecError::InvalidBracketKey(_unknown_key))
        );
    }

    #[test]
    fn test_invalid_channel_name() {
        let spec = MatchSpec::from_str("conda-forge::::foo[version=\"1.0.*\"]", Strict);
        assert_matches!(
            spec,
            Err(ParseMatchSpecError::ParseChannelError(
                ParseChannelError::InvalidName(_)
            ))
        );

        let spec = MatchSpec::from_str("conda-forge\\::foo[version=\"1.0.*\"]", Strict);
        assert_matches!(
            spec,
            Err(ParseMatchSpecError::ParseChannelError(
                ParseChannelError::InvalidName(_)
            ))
        );
    }

    #[test]
    fn test_missing_package_name() {
        for exact_names_only in [true, false] {
            let package_name = strip_package_name("", exact_names_only);
            assert_matches!(package_name, Err(ParseMatchSpecError::MissingPackageName));
        }
    }

    #[test]
    fn test_empty_namespace() {
        let spec = MatchSpec::from_str("conda-forge::foo", Strict).unwrap();
        assert!(spec.namespace.is_none());
    }

    #[test]
    fn test_multiple_semicolons() {
        // "; if" pattern should produce DeprecatedIfSyntax even with multiple semicolons
        let spec = MatchSpec::from_str("foo; if bar; if baz", Strict);
        assert_matches!(spec, Err(ParseMatchSpecError::DeprecatedIfSyntax));

        // Bare semicolons without "if" should still produce MoreThanOneSemicolon
        let spec2 = MatchSpec::from_str("package; something; else", Lenient);
        assert_matches!(spec2, Err(ParseMatchSpecError::MoreThanOneSemicolon));
    }

    #[test]
    fn test_namespace() {
        // Test with url channel and url in brackets
        let spec = MatchSpec::from_str(
            "https://a.b.c/conda-forge:namespace:foo[url=https://a.b/c/d/p-1-b_0.conda]",
            Strict,
        )
        .unwrap();
        assert_eq!(spec.namespace, Some("namespace".to_owned()));
        assert_eq!(spec.name, "foo".parse().unwrap());
        assert_eq!(spec.channel.unwrap().name(), "conda-forge");
        assert_eq!(
            spec.url,
            Some(Url::parse("https://a.b/c/d/p-1-b_0.conda").unwrap())
        );
    }

    #[test]
    fn test_parsing_url() {
        let spec = MatchSpec::from_str(
            "https://conda.anaconda.org/conda-forge/linux-64/py-rattler-0.6.1-py39h8169da8_0.conda",
            Strict,
        )
        .unwrap();

        assert_eq!(spec.url, Some(Url::parse("https://conda.anaconda.org/conda-forge/linux-64/py-rattler-0.6.1-py39h8169da8_0.conda").unwrap()));
    }

    #[test]
    fn test_parsing_path() {
        let spec = MatchSpec::from_str(
            "C:\\Users\\user\\conda-bld\\linux-64\\foo-1.0-py27_0.tar.bz2",
            Strict,
        )
        .unwrap();
        assert_eq!(
            spec.url,
            Some(
                Url::parse("file://C:/Users/user/conda-bld/linux-64/foo-1.0-py27_0.tar.bz2")
                    .unwrap()
            )
        );

        let spec = MatchSpec::from_str(
            "/home/user/conda-bld/linux-64/foo-1.0-py27_0.tar.bz2",
            Strict,
        )
        .unwrap();

        assert_eq!(
            spec.url,
            Some(Url::parse("file:/home/user/conda-bld/linux-64/foo-1.0-py27_0.tar.bz2").unwrap())
        );
    }

    #[test]
    fn test_non_happy_url_parsing() {
        let spec = MatchSpec::from_str("C:\\Users\\user\\Downloads\\package", Strict).unwrap_err();
        assert_matches!(spec, ParseMatchSpecError::MissingPackageName);

        let spec = MatchSpec::from_str("/home/user/Downloads/package", Strict).unwrap_err();
        assert_matches!(spec, ParseMatchSpecError::MissingPackageName);

        let err = MatchSpec::from_str("https://username@", Strict).expect_err("Invalid url");
        assert_eq!(err.to_string(), "invalid package spec url");

        let err = MatchSpec::from_str("bla/bla", Strict)
            .expect_err("Should try to parse as name not url");
        assert_eq!(
            err.to_string(),
            "invalid package name 'bla/bla': 'bla/bla' is not a valid package name. Package names can only contain 0-9, a-z, A-Z, -, _, or ."
        );
    }

    #[test]
    fn test_parsing_license() {
        let spec = MatchSpec::from_str("python[license=MIT]", Strict).unwrap();

        assert_eq!(spec.name, "python".parse().unwrap());
        assert_eq!(spec.license, Some("MIT".into()));
    }

    #[test]
    fn test_parsing_track_features() {
        let cases = vec![
            "python[track_features=\"pypy debug\"]",  // Space
            "python[track_features=\"pypy,debug\"]",  // Comma
            "python[track_features=\"pypy, debug\"]", // Comma + Space
        ];

        for case in cases {
            let spec = MatchSpec::from_str(case, Strict).unwrap();
            assert_eq!(
                spec.track_features,
                Some(vec!["pypy".to_string(), "debug".to_string()]),
                "Failed on syntax: {case}",
            );
        }
    }

    #[test]
    fn test_issue_717() {
        assert_matches!(
            MatchSpec::from_str("ray[default,data] >=2.9.0,<3.0.0", Strict),
            Err(ParseMatchSpecError::InvalidPackageNameMatcher(_))
        );
    }

    #[test]
    fn test_issue_736() {
        let ms1 = MatchSpec::from_str("python ==2.7.*.*|>=3.6", Lenient).expect("nameful");
        let ms2 = NamelessMatchSpec::from_str("==2.7.*.*|>=3.6", Lenient).expect("nameless");

        let (_, spec) = ms1.into_nameless();
        assert_eq!(spec, ms2);

        MatchSpec::from_str("python ==2.7.*.*|>=3.6", Strict).expect_err("nameful");
        NamelessMatchSpec::from_str("==2.7.*.*|>=3.6", Strict).expect_err("nameless");
    }

    #[test]
    fn test_parse_channel_subdir() {
        let test_cases = vec![
            ("conda-forge", Some("conda-forge"), None),
            (
                "conda-forge/linux-64",
                Some("conda-forge"),
                Some("linux-64"),
            ),
            (
                "conda-forge/label/test",
                Some("conda-forge/label/test"),
                None,
            ),
            (
                "conda-forge/linux-64/label/test",
                Some("conda-forge/linux-64/label/test"),
                None,
            ),
            ("*/linux-64", Some("*"), Some("linux-64")),
        ];

        for (input, expected_channel, expected_subdir) in test_cases {
            let (channel, subdir) = parse_channel_and_subdir(input).unwrap();
            assert_eq!(
                channel.unwrap(),
                Channel::from_str(expected_channel.unwrap(), &channel_config()).unwrap()
            );
            assert_eq!(subdir, expected_subdir.map(ToString::to_string));
        }
    }

    #[test]
    fn test_matchspec_to_string() {
        let mut specs: Vec<MatchSpec> =
            vec![MatchSpec::from_str("foo[version=1.0.*, build_number=\">6\"]", Strict).unwrap()];

        // complete matchspec to verify that we print all fields
        specs.push(MatchSpec {
            name: "foo".parse().unwrap(),
            version: Some(VersionSpec::from_str("1.0.*", Strict).unwrap()),
            build: "py27_0*".parse().ok(),
            build_number: Some(BuildNumberSpec::from_str(">=6").unwrap()),
            file_name: Some("foo-1.0-py27_0.tar.bz2".to_string()),
            extras: None,
            channel: Some(
                Channel::from_str("conda-forge", &channel_config())
                    .map(Arc::new)
                    .unwrap(),
            ),
            subdir: Some("linux-64".to_string()),
            namespace: Some("foospace".to_string()),
            md5: Some(parse_digest_from_hex::<Md5>("8b1a9953c4611296a827abf8c47804d7").unwrap()),
            sha256: Some(
                parse_digest_from_hex::<Sha256>(
                    "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3",
                )
                .unwrap(),
            ),
            url: Some(
                Url::parse(
                    "https://conda.anaconda.org/conda-forge/linux-64/foo-1.0-py27_0.tar.bz2",
                )
                .unwrap(),
            ),
            license: Some("MIT".into()),
            condition: None,
            track_features: None,
        });

        // insta check all the strings
        let vec_strings = specs.iter().map(ToString::to_string).collect::<Vec<_>>();
        insta::assert_debug_snapshot!(vec_strings);

        // parse back the strings and check if they are the same
        let parsed_specs = vec_strings
            .iter()
            .map(|s| MatchSpec::from_str(s, Strict).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(specs, parsed_specs);
    }

    #[test]
    fn test_pixi_issue_3922() {
        let match_spec = MatchSpec::from_str(
            "bird_tool_utils_python =0.*,>=0.4.1",
            ParseStrictness::Lenient,
        )
        .unwrap();
        let version_spec = match_spec.version.unwrap();
        let version = Version::from_str("0.4.1").unwrap();
        assert!(version_spec.matches(&version));
    }

    #[test]
    fn test_conditional_parsing_bracket_syntax() {
        // Basic usage with new bracket syntax
        let spec = MatchSpec::from_str(
            r#"foo[when="python >=3.6"]"#,
            ParseMatchSpecOptions::strict().with_experimental_conditionals(true),
        )
        .unwrap();
        assert_eq!(spec.name, "foo".parse().unwrap());
        assert_eq!(
            spec.condition.unwrap().to_string(),
            "python >=3.6".to_string()
        );
    }

    #[test]
    fn test_conditional_parsing_with_version() {
        // Bracket syntax with version spec
        let spec = MatchSpec::from_str(
            r#"numpy >=2.0[when="python >=3.10"]"#,
            ParseMatchSpecOptions::strict().with_experimental_conditionals(true),
        )
        .unwrap();
        assert_eq!(spec.name, "numpy".parse().unwrap());
        assert_eq!(
            spec.version,
            Some(VersionSpec::from_str(">=2.0", Strict).unwrap())
        );
        assert_eq!(
            spec.condition.unwrap().to_string(),
            "python >=3.10".to_string()
        );
    }

    #[test]
    fn test_conditional_parsing_single_quotes() {
        // Single quotes for the when value
        let spec = MatchSpec::from_str(
            r#"foo[when='python >=3.6']"#,
            ParseMatchSpecOptions::strict().with_experimental_conditionals(true),
        )
        .unwrap();
        assert_eq!(spec.name, "foo".parse().unwrap());
        assert_eq!(
            spec.condition.unwrap().to_string(),
            "python >=3.6".to_string()
        );
    }

    /// Helper to parse a conditional match spec with strict mode + conditionals enabled.
    fn parse_conditional(input: &str) -> Result<MatchSpec, ParseMatchSpecError> {
        MatchSpec::from_str(
            input,
            ParseMatchSpecOptions::strict().with_experimental_conditionals(true),
        )
    }

    #[test]
    fn test_conditional_parsing_with_and_or() {
        // Complex condition with AND/OR
        let spec = parse_conditional(r#"foo[when="python >=3.6 and linux"]"#).unwrap();
        assert_eq!(
            spec.condition.unwrap().to_string(),
            "(python >=3.6 and linux)"
        );
    }

    #[test]
    fn test_conditional_parsing_escaped_quotes_in_when_value() {
        // When value containing an inner bracket spec with quotes
        let spec = parse_conditional(r#"foo[when="python[version=\">=3.6\"]"]"#).unwrap();
        assert!(spec.condition.is_some());
        // Verify round-trip
        let reparsed = parse_conditional(&spec.to_string()).unwrap();
        assert_eq!(spec, reparsed);
    }

    #[test]
    fn test_conditional_parsing_complex_version() {
        // Complex version constraints in condition
        let spec = parse_conditional(r#"foo[when="python >=3.6,<4.0"]"#).unwrap();
        assert_eq!(spec.condition.unwrap().to_string(), "python >=3.6,<4.0");

        // Multiple conditions with or
        let spec = parse_conditional(r#"foo[when="python >=3.6 or python <3.0"]"#).unwrap();
        assert_eq!(
            spec.condition.unwrap().to_string(),
            "(python >=3.6 or python <3.0)"
        );
    }

    #[test]
    fn test_conditional_parsing_disabled() {
        // when key should be rejected when conditionals are disabled
        let spec = MatchSpec::from_str(r#"foo[when="python >=3.6"]"#, Strict);
        assert_matches!(spec, Err(ParseMatchSpecError::InvalidBracketKey(_)));
    }

    #[test]
    fn test_deprecated_if_syntax() {
        // Old "; if" syntax should return an error
        let spec = MatchSpec::from_str(
            "foo; if python >=3.6",
            ParseMatchSpecOptions::strict().with_experimental_conditionals(true),
        );
        assert_matches!(spec, Err(ParseMatchSpecError::DeprecatedIfSyntax));

        // Also without conditionals enabled
        let spec = MatchSpec::from_str("foo; if python >=3.6", Strict);
        assert_matches!(spec, Err(ParseMatchSpecError::DeprecatedIfSyntax));
    }

    #[test]
    fn test_conditional_roundtrip() {
        // Test that parsing and displaying a conditional spec produces a valid spec
        let spec = MatchSpec::from_str(
            r#"foo >=1.0[when="python >=3.6"]"#,
            ParseMatchSpecOptions::strict().with_experimental_conditionals(true),
        )
        .unwrap();

        let spec_str = spec.to_string();
        assert!(spec_str.contains(r#"when="#));

        // Parse the displayed string back
        let reparsed = MatchSpec::from_str(
            &spec_str,
            ParseMatchSpecOptions::strict().with_experimental_conditionals(true),
        )
        .unwrap();
        assert_eq!(spec, reparsed);
    }

    #[test]
    fn test_when_with_other_bracket_keys() {
        // when key combined with other bracket keys
        let spec =
            parse_conditional(r#"foo[version=">=1.0", when="python >=3.6", build="py*"]"#).unwrap();
        assert_eq!(spec.name, "foo".parse().unwrap());
        assert_eq!(
            spec.version,
            Some(VersionSpec::from_str(">=1.0", Strict).unwrap())
        );
        assert_eq!(spec.condition.unwrap().to_string(), "python >=3.6");
        assert_eq!(spec.build.unwrap().to_string(), "py*");
    }

    #[test]
    fn test_unescape_string() {
        use std::borrow::Cow;

        // No escapes - should return borrowed
        let result = unescape_string("hello world");
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(result, "hello world");

        // Escaped double quote
        let result = unescape_string(r#"hello \"world\""#);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(result, r#"hello "world""#);

        // Escaped single quote
        let result = unescape_string(r"hello \'world\'");
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(result, "hello 'world'");

        // Escaped backslash
        let result = unescape_string(r"hello \\world");
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(result, r"hello \world");

        // Mixed escapes
        let result = unescape_string(r#"\"test\' \\"#);
        assert_eq!(result, r#""test' \"#);

        // Backslash followed by non-escape character (kept as-is)
        let result = unescape_string(r"hello \n world");
        assert_eq!(result, r"hello \n world");
    }

    #[test]
    fn test_bracket_value_with_escaped_quotes() {
        // Test that bracket values with escaped quotes are parsed correctly
        let result = strip_brackets(r#"foo[version="1.0", build="py\"37\"_0"]"#).unwrap();
        assert_eq!(result.0, "foo");
        assert_eq!(result.1.len(), 2);
        assert_eq!(result.1[0], ("version", "1.0"));
        // The value should still contain the escape sequences
        assert_eq!(result.1[1], ("build", r#"py\"37\"_0"#));
    }

    #[test]
    fn test_nested_when_conditions_not_allowed() {
        // According to the CEP, inner MatchSpec queries MUST NOT feature their own `when` field.
        // The inner condition parser uses strict mode without experimental conditionals,
        // so nested when conditions should fail with an InvalidCondition error.

        // Test case 1: Simple nested when
        let spec = MatchSpec::from_str(
            r#"foo[when="bar[when=\"baz\"]"]"#,
            ParseMatchSpecOptions::strict().with_experimental_conditionals(true),
        );
        assert!(spec.is_err());
        let err = spec.unwrap_err();
        // Should be an InvalidCondition error because the inner parser rejects `when` key
        assert_matches!(err, ParseMatchSpecError::InvalidCondition(_, _));

        // Test case 2: Nested when in OR condition
        let spec = MatchSpec::from_str(
            r#"foo[when="bar or baz[when=\"qux\"]"]"#,
            ParseMatchSpecOptions::strict().with_experimental_conditionals(true),
        );
        assert!(spec.is_err());
        assert_matches!(
            spec.unwrap_err(),
            ParseMatchSpecError::InvalidCondition(_, _)
        );

        // Test case 3: Nested when in AND condition
        let spec = MatchSpec::from_str(
            r#"foo[when="bar and baz[when=\"qux\"]"]"#,
            ParseMatchSpecOptions::strict().with_experimental_conditionals(true),
        );
        assert!(spec.is_err());
        assert_matches!(
            spec.unwrap_err(),
            ParseMatchSpecError::InvalidCondition(_, _)
        );

        // Test case 4: Deeply nested when (when inside when inside when)
        let spec = MatchSpec::from_str(
            r#"foo[when="bar[when=\"baz[when=\\\"qux\\\"]\"]"]"#,
            ParseMatchSpecOptions::strict().with_experimental_conditionals(true),
        );
        assert!(spec.is_err());
        assert_matches!(
            spec.unwrap_err(),
            ParseMatchSpecError::InvalidCondition(_, _)
        );
    }

    #[test]
    fn test_conditional_empty_when_value() {
        // Empty when value should error
        let spec = parse_conditional(r#"foo[when=""]"#);
        assert!(spec.is_err());

        // Whitespace-only when value should error
        let spec = parse_conditional(r#"foo[when="   "]"#);
        assert!(spec.is_err());
    }

    #[test]
    fn test_conditional_multiple_when_keys() {
        // Multiple when keys in strict mode should error
        let spec = MatchSpec::from_str(
            r#"foo[when="a", when="b"]"#,
            ParseMatchSpecOptions::strict().with_experimental_conditionals(true),
        );
        assert_matches!(spec, Err(ParseMatchSpecError::MultipleValueForKey(_)));
    }

    #[test]
    fn test_conditional_package_name_with_and_or_substring() {
        // Package names containing "and"/"or" substrings should not be split
        let spec = parse_conditional(r#"foo[when="pandoc >=2.0"]"#).unwrap();
        assert_eq!(spec.condition.unwrap().to_string(), "pandoc >=2.0");
    }

    #[test]
    fn test_conditional_roundtrip_and_or() {
        // Round-trip with AND condition
        let spec = parse_conditional(r#"foo[when="python >=3.6 and __linux"]"#).unwrap();
        let reparsed = parse_conditional(&spec.to_string()).unwrap();
        assert_eq!(spec, reparsed);

        // Round-trip with OR condition
        let spec = parse_conditional(r#"foo[when="python >=3.6 or python <3.0"]"#).unwrap();
        let reparsed = parse_conditional(&spec.to_string()).unwrap();
        assert_eq!(spec, reparsed);
    }

    #[test]
    fn test_conditional_roundtrip_parenthesized() {
        // Round-trip with parenthesized conditions
        let spec =
            parse_conditional(r#"foo[when="(python >=3.6 or python <3.0) and __unix"]"#).unwrap();
        let reparsed = parse_conditional(&spec.to_string()).unwrap();
        assert_eq!(spec, reparsed);
    }

    #[test]
    fn test_nameless_match_spec_with_when() {
        // NamelessMatchSpec with when should work
        let spec = NamelessMatchSpec::from_str(
            r#">=1.0[when="python >=3.6"]"#,
            ParseMatchSpecOptions::strict().with_experimental_conditionals(true),
        )
        .unwrap();
        assert_eq!(
            spec.version,
            Some(VersionSpec::from_str(">=1.0", Strict).unwrap())
        );
        assert_eq!(spec.condition.unwrap().to_string(), "python >=3.6");
    }

    #[test]
    fn test_when_rejected_without_conditionals_lenient() {
        // when key should be rejected in Lenient mode when conditionals disabled
        let spec = MatchSpec::from_str(r#"foo[when="a"]"#, Lenient);
        assert_matches!(spec, Err(ParseMatchSpecError::InvalidBracketKey(_)));
    }

    #[test]
    fn test_nameless_deprecated_if_syntax() {
        // NamelessMatchSpec with deprecated ; if syntax should error
        let spec = NamelessMatchSpec::from_str("; if python >=3.6", Strict);
        assert_matches!(spec, Err(ParseMatchSpecError::DeprecatedIfSyntax));

        let spec = NamelessMatchSpec::from_str(">=1.0; if python >=3.6", Strict);
        assert_matches!(spec, Err(ParseMatchSpecError::DeprecatedIfSyntax));
    }

    #[test]
    fn test_conditional_inner_bracket_spec() {
        // Inner bracket specs in conditions should work with proper bracket syntax
        let spec = parse_conditional(r#"foo[when="python[version=\">=3.6\"]"]"#).unwrap();
        assert!(spec.condition.is_some());
    }

    #[test]
    fn test_simple_extras() {
        let spec = MatchSpec::from_str(
            "foo[extras=[bar]]",
            ParseMatchSpecOptions::strict().with_experimental_extras(true),
        )
        .unwrap();

        assert_eq!(spec.extras, Some(vec!["bar".to_string()]));
        assert!(MatchSpec::from_str(
            "foo[extras=[bar,baz]",
            ParseMatchSpecOptions::strict().with_experimental_extras(true)
        )
        .is_err());
    }

    #[test]
    fn test_multiple_extras() {
        let spec = MatchSpec::from_str(
            "foo[extras=[bar,baz]]",
            ParseMatchSpecOptions::strict().with_experimental_extras(true),
        )
        .unwrap();
        assert_eq!(
            spec.extras,
            Some(vec!["bar".to_string(), "baz".to_string()])
        );
    }

    #[test]
    fn test_parse_extras() {
        assert_eq!(
            parse_extras("bar,baz").unwrap(),
            vec!["bar".to_string(), "baz".to_string()]
        );
        assert_eq!(parse_extras("bar").unwrap(), vec!["bar".to_string()]);
        assert_eq!(
            parse_extras("bar, baz").unwrap(),
            vec!["bar".to_string(), "baz".to_string()]
        );
        assert!(parse_extras("[bar,baz]").is_err());
    }

    #[test]
    fn test_invalid_extras() {
        let opts = ParseMatchSpecOptions::strict().with_experimental_extras(true);

        // Empty extras value
        assert!(MatchSpec::from_str("foo[extras=]", opts).is_err());

        // Missing brackets around extras list
        assert!(MatchSpec::from_str("foo[extras=bar,baz]", opts).is_err());

        // Trailing comma in extras list
        assert!(MatchSpec::from_str("foo[extras=[bar,]]", opts).is_err());

        // Invalid characters in extras name
        assert!(MatchSpec::from_str("foo[extras=[bar!,baz]]", opts).is_err());

        // Invalid characters in extras name
        println!("{:?}", MatchSpec::from_str("foo[extras=[bar!,baz]]", opts));
        assert!(MatchSpec::from_str("foo[extras=[bar!,baz]]", opts).is_err());

        // Empty extras item
        assert!(MatchSpec::from_str("foo[extras=[bar,,baz]]", opts).is_err());

        // Missing closing bracket
        assert!(MatchSpec::from_str("foo[extras=[bar,baz", opts).is_err());

        // Missing opening bracket
        assert!(MatchSpec::from_str("foo[extras=bar,baz]]", opts).is_err());
    }

    #[test]
    fn test_glob_and_regex_error_messages() {
        // Test glob error message
        let glob_err = MatchSpec::from_str("bla*", Strict).unwrap_err();
        assert_eq!(
            glob_err.to_string(),
            "\"bla*\" looks like a glob but only exact package names are allowed, package names can only contain 0-9, a-z, A-Z, -, _, or ."
        );

        // Test regex error message
        let regex_err = MatchSpec::from_str("^foo.*$", Strict).unwrap_err();
        assert_eq!(
            regex_err.to_string(),
            "\"^foo.*$\" looks like a regex but only exact package names are allowed, package names can only contain 0-9, a-z, A-Z, -, _, or ."
        );
    }
}
