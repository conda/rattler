use std::{borrow::Cow, collections::HashSet, ops::Not, str::FromStr, sync::Arc};

use nom::{
    branch::alt,
    bytes::complete::{tag, take_till1, take_until, take_while, take_while1},
    character::complete::{char, multispace0, one_of, space0},
    combinator::{opt, recognize},
    error::{context, ContextError, ParseError},
    multi::{separated_list0, separated_list1},
    sequence::{delimited, preceded, separated_pair, terminated},
    Finish, IResult,
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
use crate::{
    build_spec::{BuildNumberSpec, ParseBuildNumberSpecError},
    package::ArchiveIdentifier,
    utils::{path::is_absolute_path, url::parse_scheme},
    version_spec::{
        is_start_of_version_constraint,
        version_tree::{recognize_constraint, recognize_version},
        ParseVersionSpecError,
    },
    Channel, ChannelConfig, InvalidPackageNameError, NamelessMatchSpec, PackageName,
    ParseChannelError, ParseStrictness,
    ParseStrictness::{Lenient, Strict},
    ParseVersionError, Platform, VersionSpec,
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
    #[error("the build string '{0}' is not valid, it can only contain alphanumeric characters and underscores"
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

    /// The package name was invalid
    #[error(transparent)]
    InvalidPackageName(#[from] InvalidPackageNameError),

    /// Multiple values for a key in the matchspec
    #[error("found multiple values for: {0}")]
    MultipleValueForKey(String),
}

impl FromStr for MatchSpec {
    type Err = ParseMatchSpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str(s, Lenient)
    }
}

impl MatchSpec {
    /// Parses a [`MatchSpec`] from a string with a given strictness.
    pub fn from_str(
        source: &str,
        strictness: ParseStrictness,
    ) -> Result<Self, ParseMatchSpecError> {
        matchspec_parser(source, strictness)
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

/// Strips any if statements from the matchspec. `if` statements in matchspec
/// are "anticipating future compatibility issues".
fn strip_if(input: &str) -> (&str, Option<&str>) {
    // input
    //     .split_once("if")
    //     .map(|(spec, if_statement)| (spec, Some(if_statement)))
    //     .unwrap_or_else(|| (input, None))
    (input, None)
}

/// An optimized data structure to store key value pairs in between a bracket
/// string `[key1=value1, key2=value2]`. The optimization stores two such values
/// on the stack and otherwise allocates a vector on the heap. Two is chosen
/// because that seems to be more than enough for most use cases.
type BracketVec<'a> = SmallVec<[(&'a str, &'a str); 2]>;

/// A parse combinator to filter whitespace if front and after another parser.
fn whitespace_enclosed<'a, F, O, E: ParseError<&'a str>>(
    mut inner: F,
) -> impl FnMut(&'a str) -> IResult<&'a str, O, E>
where
    F: FnMut(&'a str) -> IResult<&'a str, O, E>,
{
    move |input: &'a str| {
        let (input, _) = multispace0(input)?;
        let (input, o2) = inner(input)?;
        multispace0(input).map(|(i, _)| (i, o2))
    }
}

/// Parses the contents of a bracket list `[version="1,2,3", bla=3]`
fn parse_bracket_list(input: &str) -> Result<BracketVec<'_>, ParseMatchSpecError> {
    /// Parses a key in a bracket string
    fn parse_key(input: &str) -> IResult<&str, &str> {
        whitespace_enclosed(context(
            "key",
            take_while(|c: char| c.is_alphanumeric() || c == '_' || c == '-'),
        ))(input)
    }

    /// Parses a value in a bracket string.
    fn parse_value(input: &str) -> IResult<&str, &str> {
        whitespace_enclosed(context(
            "value",
            alt((
                delimited(char('"'), take_until("\""), char('"')),
                delimited(char('\''), take_until("'"), char('\'')),
                take_till1(|c| c == ',' || c == ']' || c == '\'' || c == '"'),
            )),
        ))(input)
    }

    /// Parses a `key=value` pair
    fn parse_key_value(input: &str) -> IResult<&str, (&str, &str)> {
        separated_pair(parse_key, char('='), parse_value)(input)
    }

    /// Parses a list of `key=value` pairs separated by commas
    fn parse_key_value_list(input: &str) -> IResult<&str, Vec<(&str, &str)>> {
        separated_list0(whitespace_enclosed(char(',')), parse_key_value)(input)
    }

    /// Parses an entire bracket string
    fn parse_bracket_list(input: &str) -> IResult<&str, Vec<(&str, &str)>> {
        delimited(char('['), parse_key_value_list, char(']'))(input)
    }

    match parse_bracket_list(input).finish() {
        Ok((_remaining, values)) => Ok(values.into()),
        Err(nom::error::Error { .. }) => Err(ParseMatchSpecError::InvalidBracket),
    }
}

/// Strips the brackets part of the matchspec returning the rest of the
/// matchspec and  the contents of the brackets as a `Vec<&str>`.
fn strip_brackets(input: &str) -> Result<(Cow<'_, str>, BracketVec<'_>), ParseMatchSpecError> {
    if let Some(matches) = lazy_regex::regex!(r#".*(?:(\[.*\]))$"#).captures(input) {
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

/// Parses a [`BracketVec`] into precise components
fn parse_bracket_vec_into_components(
    bracket: BracketVec<'_>,
    match_spec: NamelessMatchSpec,
    strictness: ParseStrictness,
) -> Result<NamelessMatchSpec, ParseMatchSpecError> {
    let mut match_spec = match_spec;

    if strictness == Strict {
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
            "version" => match_spec.version = Some(VersionSpec::from_str(value, strictness)?),
            "build" => match_spec.build = Some(StringMatcher::from_str(value)?),
            "build_number" => match_spec.build_number = Some(BuildNumberSpec::from_str(value)?),
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
            // TODO: Still need to add `track_features`, `features`, `license` and `license_family`
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
fn strip_package_name(input: &str) -> Result<(PackageName, &str), ParseMatchSpecError> {
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

    Ok((PackageName::from_str(trimmed_package_name)?, rest.trim()))
}

/// Splits a string into version and build constraints.
fn split_version_and_build(
    input: &str,
    strictness: ParseStrictness,
) -> Result<(&str, Option<&str>), ParseMatchSpecError> {
    fn parse_version_constraint_or_group<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
        input: &'a str,
    ) -> IResult<&'a str, &'a str, E> {
        alt((
            delimited(tag("("), parse_version_group, tag(")")),
            recognize_constraint,
        ))(input)
    }

    fn parse_version_group<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
        input: &'a str,
    ) -> IResult<&'a str, &'a str, E> {
        recognize(separated_list1(
            whitespace_enclosed(one_of(",|")),
            parse_version_constraint_or_group,
        ))(input)
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
        ))(input)
    }

    fn parse_version_and_build_separator<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
        strictness: ParseStrictness,
    ) -> impl FnMut(&'a str) -> IResult<&'a str, &'a str, E> {
        move |input: &'a str| {
            if strictness == Lenient {
                terminated(
                    alt((parse_special_equality, parse_version_group)),
                    opt(one_of(" =")),
                )(input)
            } else {
                terminated(parse_version_group, space0)(input)
            }
        }
    }

    match parse_version_and_build_separator(strictness)(input).finish() {
        Ok((rest, version)) => {
            let build_string = rest.trim();

            // Check validity of the build string
            if strictness == Strict
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
        Err(nom::error::VerboseError { .. }) => Err(ParseMatchSpecError::InvalidVersionAndBuild(
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
        Self::from_str(input, Lenient)
    }
}

impl NamelessMatchSpec {
    /// Parses a [`NamelessMatchSpec`] from a string with a given strictness.
    pub fn from_str(input: &str, strictness: ParseStrictness) -> Result<Self, ParseMatchSpecError> {
        // Strip off brackets portion
        let (input, brackets) = strip_brackets(input.trim())?;
        let input = input.trim();

        // Parse url or path spec
        if let Some(url) = parse_url_like(input)? {
            return Ok(NamelessMatchSpec {
                url: Some(url),
                ..NamelessMatchSpec::default()
            });
        }

        let mut match_spec =
            parse_bracket_vec_into_components(brackets, NamelessMatchSpec::default(), strictness)?;

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
            let (version, build) = parse_version_and_build(input, strictness)?;
            if strictness == Strict {
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
fn matchspec_parser(
    input: &str,
    strictness: ParseStrictness,
) -> Result<MatchSpec, ParseMatchSpecError> {
    // Step 1. Strip '#' and `if` statement
    let (input, _comment) = strip_comment(input);
    let (input, _if_clause) = strip_if(input);

    // 2. Strip off brackets portion
    let (input, brackets) = strip_brackets(input.trim())?;
    let mut nameless_match_spec =
        parse_bracket_vec_into_components(brackets, NamelessMatchSpec::default(), strictness)?;

    // 3. Strip off parens portion
    // TODO: What is this? I've never seen it

    // 4. Parse as url
    if nameless_match_spec.url.is_none() {
        if let Some(url) = parse_url_like(&input)? {
            let archive = ArchiveIdentifier::try_from_url(&url);
            let name = archive.and_then(|a| a.try_into().ok());

            // TODO: This should also work without a proper name from the url filename
            if name.is_none() {
                return Err(ParseMatchSpecError::MissingPackageName);
            }

            // Only return the 'url' and 'name' to avoid miss parsing the rest of the
            // information. e.g. when a version is provided in the url is not the
            // actual version this might be a problem when solving.
            return Ok(MatchSpec {
                url: Some(url),
                name,
                ..MatchSpec::default()
            });
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
    let (name, input) = strip_package_name(input)?;
    let mut match_spec = MatchSpec::from_nameless(nameless_match_spec, Some(name));

    // Step 7. Otherwise, sort our version + build
    let input = input.trim();
    if !input.is_empty() {
        let (version, build) = parse_version_and_build(input, strictness)?;
        if strictness == Strict {
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
    if strictness != Lenient {
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
        BracketVec, MatchSpec, ParseMatchSpecError,
    };
    use crate::{
        match_spec::parse::parse_bracket_list, BuildNumberSpec, Channel, ChannelConfig,
        NamelessMatchSpec, ParseChannelError, ParseStrictness, ParseStrictness::*, VersionSpec,
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
            NamelessMatchSpec::from_str("1.0 py27_0[fn=\"bla\"]", Strict).unwrap(),
            NamelessMatchSpec::from_str("=1.0 py27_0", Strict).unwrap(),
            NamelessMatchSpec::from_str("*cpu*", Strict).unwrap(),
            NamelessMatchSpec::from_str("conda-forge::foobar", Strict).unwrap(),
            NamelessMatchSpec::from_str("foobar[channel=conda-forge]", Strict).unwrap(),
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
            base_url: "https://conda.anaconda.org/conda-forge/"
            name: conda-forge
        - version: "==foobar"
          channel:
            base_url: "https://conda.anaconda.org/conda-forge/"
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
        assert_eq!(spec.name, Some("foo".parse().unwrap()));
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
        assert_eq!(spec.name, Some("foo".parse().unwrap()));
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
        assert_eq!(spec.name, Some("foo".parse().unwrap()));
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
            MatchSpec(MatchSpec),
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
                        MatchSpecOrError::MatchSpec,
                    ),
                )
            })
            .collect();

        // Strip absolute paths to this crate from the channels for testing
        let crate_root = env!("CARGO_MANIFEST_DIR");
        let crate_path = Url::from_directory_path(std::path::Path::new(crate_root)).unwrap();
        let home = Url::from_directory_path(dirs::home_dir().unwrap()).unwrap();
        insta::with_settings!({filters => vec![
            (crate_path.as_str(), "file://<CRATE>/"),
            (home.as_str(), "file://<HOME>/"),
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
            Spec(NamelessMatchSpec),
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
                        NamelessSpecOrError::Spec,
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
        let package_name = strip_package_name("");
        assert_matches!(package_name, Err(ParseMatchSpecError::MissingPackageName));
    }

    #[test]
    fn test_empty_namespace() {
        let spec = MatchSpec::from_str("conda-forge::foo", Strict).unwrap();
        assert!(spec.namespace.is_none());
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
        assert_eq!(spec.name, Some("foo".parse().unwrap()));
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
        assert_eq!(err.to_string(), "'bla/bla' is not a valid package name. Package names can only contain 0-9, a-z, A-Z, -, _, or .");
    }

    #[test]
    fn test_issue_717() {
        assert_matches!(
            MatchSpec::from_str("ray[default,data] >=2.9.0,<3.0.0", Strict),
            Err(ParseMatchSpecError::InvalidPackageName(_))
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
            name: Some("foo".parse().unwrap()),
            version: Some(VersionSpec::from_str("1.0.*", Strict).unwrap()),
            build: "py27_0*".parse().ok(),
            build_number: Some(BuildNumberSpec::from_str(">=6").unwrap()),
            file_name: Some("foo-1.0-py27_0.tar.bz2".to_string()),
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
}
