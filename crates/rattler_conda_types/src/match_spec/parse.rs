use std::{borrow::Cow, path::PathBuf, str::FromStr};

use nom::{
    branch::alt,
    bytes::complete::{tag, take_till1, take_until, take_while, take_while1},
    character::complete::{char, multispace0, one_of},
    combinator::{opt, recognize},
    error::{context, ContextError, ParseError},
    multi::{separated_list0, separated_list1},
    sequence::{delimited, preceded, separated_pair, terminated},
    Finish, IResult,
};
use rattler_digest::{parse_digest_from_hex, Md5, Sha256};
use smallvec::SmallVec;
use thiserror::Error;
use url::Url;

use super::{
    matcher::{StringMatcher, StringMatcherParseError},
    MatchSpec,
};
use crate::{
    build_spec::{BuildNumberSpec, ParseBuildNumberSpecError},
    package::ArchiveType,
    version_spec::{
        is_start_of_version_constraint,
        version_tree::{recognize_constraint, recognize_version},
        ParseVersionSpecError,
    },
    Channel, ChannelConfig, InvalidPackageNameError, NamelessMatchSpec, PackageName,
    ParseChannelError, ParseStrictness, VersionSpec,
};

/// The type of parse error that occurred when parsing match spec.
#[derive(Debug, Clone, Error)]
pub enum ParseMatchSpecError {
    /// The path or url of the package was invalid
    #[error("invalid package path or url")]
    InvalidPackagePathOrUrl,

    /// Invalid bracket in match spec
    #[error("invalid bracket")]
    InvalidBracket,

    /// Invalid number of colons in match spec
    #[error("invalid number of colons")]
    InvalidNumberOfColons,

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
    #[error("Unable to parse version spec: {0}")]
    InvalidVersionAndBuild(String),

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
    #[error("Unable to parse hash digest from hex")]
    InvalidHashDigest,

    /// The package name was invalid
    #[error(transparent)]
    InvalidPackageName(#[from] InvalidPackageNameError),
}

impl FromStr for MatchSpec {
    type Err = ParseMatchSpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str(s, ParseStrictness::Lenient)
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

/// Returns true if the specified string represents a package path.
fn is_package_file(input: &str) -> bool {
    ArchiveType::try_from(input).is_some()
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

    /// Parses a list of `key=value` pairs seperate by commas
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
    if let Some(matches) = lazy_regex::regex!(r#".*(?:(\[.*\]))"#).captures(input) {
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
            _ => Err(ParseMatchSpecError::InvalidBracketKey(key.to_owned()))?,
        }
    }

    Ok(match_spec)
}

/// Strip the package name from the input.
fn strip_package_name(input: &str) -> Result<(PackageName, &str), ParseMatchSpecError> {
    match take_while1(|c: char| !c.is_whitespace() && !is_start_of_version_constraint(c))(input)
        .finish()
    {
        Ok((input, name)) => Ok((PackageName::from_str(name.trim())?, input.trim())),
        Err(nom::error::Error { .. }) => Err(ParseMatchSpecError::MissingPackageName),
    }
}

/// Splits a string into version and build constraints.
fn split_version_and_build(input: &str) -> Result<(&str, Option<&str>), ParseMatchSpecError> {
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

    fn parse_version_and_build_seperator<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
        input: &'a str,
    ) -> IResult<&'a str, &'a str, E> {
        terminated(
            alt((parse_special_equality, parse_version_group)),
            opt(one_of(" =")),
        )(input)
    }

    match parse_version_and_build_seperator(input).finish() {
        Ok((rest, version)) => {
            let build_number = rest.trim();
            Ok((
                version.trim(),
                if build_number.is_empty() {
                    None
                } else {
                    Some(build_number)
                },
            ))
        }
        Err(e @ nom::error::VerboseError { .. }) => {
            eprintln!("{}", nom::error::convert_error(input, e));
            Err(ParseMatchSpecError::InvalidVersionAndBuild(
                input.to_string(),
            ))
        }
    }
}

impl FromStr for NamelessMatchSpec {
    type Err = ParseMatchSpecError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::from_str(input, ParseStrictness::Lenient)
    }
}

impl NamelessMatchSpec {
    /// Parses a [`NamelessMatchSpec`] from a string with a given strictness.
    pub fn from_str(input: &str, strictness: ParseStrictness) -> Result<Self, ParseMatchSpecError> {
        // Strip off brackets portion
        let (input, brackets) = strip_brackets(input.trim())?;
        let mut match_spec =
            parse_bracket_vec_into_components(brackets, NamelessMatchSpec::default(), strictness)?;

        // Get the version and optional build string
        let input = input.trim();
        if !input.is_empty() {
            if input.find('[').is_some() {
                return Err(ParseMatchSpecError::MultipleBracketSectionsNotAllowed);
            }

            let (version_str, build_str) = split_version_and_build(input)?;

            let version_str = if version_str.find(char::is_whitespace).is_some() {
                Cow::Owned(version_str.replace(char::is_whitespace, ""))
            } else {
                Cow::Borrowed(version_str)
            };

            // Parse the version spec
            match_spec.version = Some(
                VersionSpec::from_str(version_str.as_ref(), strictness)
                    .map_err(ParseMatchSpecError::InvalidVersionSpec)?,
            );

            if let Some(build) = build_str {
                match_spec.build = Some(StringMatcher::from_str(build)?);
            }
        }

        Ok(match_spec)
    }
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

    // 2. Is the spec a tarball?
    if is_package_file(input) {
        let _url = match Url::parse(input) {
            Ok(url) => url,
            #[cfg(target_arch = "wasm32")]
            Err(_) => return Err(ParseMatchSpecError::InvalidPackagePathOrUrl),
            #[cfg(not(target_arch = "wasm32"))]
            Err(_) => match PathBuf::from_str(input) {
                Ok(path) => Url::from_file_path(path)
                    .map_err(|_err| ParseMatchSpecError::InvalidPackagePathOrUrl)?,
                Err(_) => return Err(ParseMatchSpecError::InvalidPackagePathOrUrl),
            },
        };

        // TODO: Implementing package file specs
        unimplemented!()
    }

    // 3. Strip off brackets portion
    let (input, brackets) = strip_brackets(input.trim())?;
    let mut nameless_match_spec =
        parse_bracket_vec_into_components(brackets, NamelessMatchSpec::default(), strictness)?;

    // 4. Strip off parens portion
    // TODO: What is this? I've never seen in

    // 5. Strip of '::' channel and namespace
    let mut input_split = input.split(':').fuse();
    let (input, namespace, channel_str) = match (
        input_split.next(),
        input_split.next(),
        input_split.next(),
        input_split.next(),
    ) {
        (Some(input), None, _, _) => (input, None, None),
        (Some(namespace), Some(input), None, _) => (input, Some(namespace), None),
        (Some(channel_str), Some(namespace), Some(input), None) => {
            (input, Some(namespace), Some(channel_str))
        }
        _ => return Err(ParseMatchSpecError::InvalidNumberOfColons),
    };

    nameless_match_spec.namespace = namespace
        .map(str::trim)
        .filter(|namespace| !namespace.is_empty())
        .map(ToOwned::to_owned)
        .or(nameless_match_spec.namespace);

    if let Some(channel_str) = channel_str {
        let channel_config = ChannelConfig::default_with_root_dir(
            std::env::current_dir().expect("Could not get current directory"),
        );
        if let Some((channel, subdir)) = channel_str.rsplit_once('/') {
            nameless_match_spec.channel = Some(Channel::from_str(channel, &channel_config)?.into());
            nameless_match_spec.subdir = Some(subdir.to_string());
        } else {
            nameless_match_spec.channel =
                Some(Channel::from_str(channel_str, &channel_config)?.into());
        }
    }

    // Step 6. Strip off the package name from the input
    let (name, input) = strip_package_name(input)?;
    let mut match_spec = MatchSpec::from_nameless(nameless_match_spec, Some(name));

    // Step 7. Otherwise sort our version + build
    let input = input.trim();
    if !input.is_empty() {
        if input.find('[').is_some() {
            return Err(ParseMatchSpecError::MultipleBracketSectionsNotAllowed);
        }

        let (version_str, build_str) = split_version_and_build(input)?;

        let version_str = if version_str.find(char::is_whitespace).is_some() {
            Cow::Owned(version_str.replace(char::is_whitespace, ""))
        } else {
            Cow::Borrowed(version_str)
        };

        // Special case handling for version strings that start with `=`.
        let version_str = if let (Some(version_str), true) =
            (version_str.strip_prefix("=="), build_str.is_none())
        {
            // If the version starts with `==` and the build string is none we strip the
            // `==` part.
            Cow::Borrowed(version_str)
        } else if let Some(version_str_part) = version_str.strip_prefix('=') {
            let not_a_group = !version_str_part.contains(['=', ',', '|']);
            if not_a_group {
                // If the version starts with `=`, is not part of a group (e.g. 1|2) we append a
                // * if it doesnt have one already.
                if build_str.is_none() && !version_str_part.ends_with('*') {
                    Cow::Owned(format!("{version_str_part}*"))
                } else {
                    Cow::Borrowed(version_str_part)
                }
            } else {
                // Version string is part of a group, return the non-stripped version string
                version_str
            }
        } else {
            version_str
        };

        // Parse the version spec
        match_spec.version = Some(
            VersionSpec::from_str(version_str.as_ref(), strictness)
                .map_err(ParseMatchSpecError::InvalidVersionSpec)?,
        );

        if let Some(build) = build_str {
            match_spec.build = Some(StringMatcher::from_str(build)?);
        }
    }

    Ok(match_spec)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, str::FromStr, sync::Arc};

    use assert_matches::assert_matches;
    use rattler_digest::{parse_digest_from_hex, Md5, Sha256};
    use serde::Serialize;
    use smallvec::smallvec;

    use super::{
        split_version_and_build, strip_brackets, strip_package_name, BracketVec, MatchSpec,
        ParseMatchSpecError,
    };
    use crate::{
        match_spec::parse::parse_bracket_list, BuildNumberSpec, Channel, ChannelConfig,
        NamelessMatchSpec, ParseStrictness::*, VersionSpec,
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
            split_version_and_build("==1.0=py27_0"),
            Ok(("==1.0", Some("py27_0")))
        );
        assert_matches!(split_version_and_build("=*=cuda"), Ok(("=*", Some("cuda"))));
        assert_matches!(
            split_version_and_build("=1.2.3 0"),
            Ok(("=1.2.3", Some("0")))
        );
        assert_matches!(split_version_and_build("1.2.3=0"), Ok(("1.2.3", Some("0"))));
        assert_matches!(
            split_version_and_build(">=1.0 , < 2.0 py34_0"),
            Ok((">=1.0 , < 2.0", Some("py34_0")))
        );
        assert_matches!(
            split_version_and_build(">=1.0 , < 2.0 =py34_0"),
            Ok((">=1.0 , < 2.0", Some("=py34_0")))
        );
        assert_matches!(split_version_and_build("=1.2.3 "), Ok(("=1.2.3", None)));
        assert_matches!(
            split_version_and_build(">1.8,<2|==1.7"),
            Ok((">1.8,<2|==1.7", None))
        );
        assert_matches!(
            split_version_and_build("* openblas_0"),
            Ok(("*", Some("openblas_0")))
        );
        assert_matches!(split_version_and_build("* *"), Ok(("*", Some("*"))));
        assert_matches!(
            split_version_and_build(">=1!164.3095,<1!165"),
            Ok((">=1!164.3095,<1!165", None))
        );
    }

    #[test]
    fn test_nameless_match_spec() {
        insta::assert_yaml_snapshot!([
            NamelessMatchSpec::from_str("3.8.* *_cpython", Strict).unwrap(),
            NamelessMatchSpec::from_str("1.0 py27_0[fn=\"bla\"]", Strict).unwrap(),
            NamelessMatchSpec::from_str("=1.0 py27_0", Strict).unwrap(),
        ],
        @r###"
        ---
        - version: 3.8.*
          build: "*_cpython"
        - version: "==1.0"
          build: py27_0
          file_name: bla
        - version: 1.0.*
          build: py27_0
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

    #[test]
    fn test_from_str() {
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
            "foo=1.0=py27_0",
            "foo==1.0=py27_0",
            "python 3.8.* *_cpython",
            "pytorch=*=cuda*",
            "x264 >=1!164.3095,<1!165",
        ];

        let evaluated: BTreeMap<_, _> = specs
            .iter()
            .map(|spec| {
                (
                    spec,
                    MatchSpec::from_str(spec, Strict).map_or_else(
                        |err| MatchSpecOrError::Error {
                            error: err.to_string(),
                        },
                        MatchSpecOrError::MatchSpec,
                    ),
                )
            })
            .collect();
        insta::assert_yaml_snapshot!("parsed matchspecs", evaluated);
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
    fn test_invalid_number_of_colons() {
        let spec = MatchSpec::from_str("conda-forge::::foo[version=\"1.0.*\"]", Strict);
        assert_matches!(spec, Err(ParseMatchSpecError::InvalidNumberOfColons));
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
}
