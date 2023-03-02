use super::MatchSpec;
use crate::version_spec::{is_start_of_version_constraint, ParseVersionSpecError};
use crate::{Channel, ChannelConfig, ParseChannelError, VersionSpec};
use nom::branch::alt;
use nom::bytes::complete::{tag, take_till1, take_until, take_while, take_while1};
use nom::character::complete::{char, multispace0, one_of};
use nom::combinator::{opt, recognize};
use nom::error::{context, ParseError};
use nom::multi::{separated_list0, separated_list1};
use nom::sequence::{delimited, pair, separated_pair, terminated};
use nom::{Finish, IResult};
use smallvec::SmallVec;
use std::borrow::Cow;
use std::num::ParseIntError;
use std::path::PathBuf;
use std::str::FromStr;
use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, Eq, PartialEq, Error)]
pub enum ParseMatchSpecError {
    #[error("invalid package path or url")]
    InvalidPackagePathOrUrl,

    #[error("invalid bracket")]
    InvalidBracket,

    #[error("invalid number of colons")]
    InvalidNumberOfColons,

    #[error("invalid channel")]
    ParseChannelError(#[from] ParseChannelError),

    #[error("invalid bracket key: {0}")]
    InvalidBracketKey(String),

    #[error("missing package name")]
    MissingPackageName,

    #[error("multiple bracket sections not allowed")]
    MultipleBracketSectionsNotAllowed,

    #[error("invalid version and build")]
    InvalidVersionAndBuild,

    #[error("invalid version spec: {0}")]
    InvalidVersionSpec(#[from] ParseVersionSpecError),

    #[error("invalid build number: {0}")]
    InvalidBuildNumber(#[from] ParseIntError),
}

impl MatchSpec {
    /// Parses a matchspec from a string.
    pub fn from_str(input: &str) -> Result<MatchSpec, ParseMatchSpecError> {
        parse(input)
    }
}

/// Strips a comment from a match spec. A comment is preceded by a '#' followed by the comment
/// itself. This functions splits the matchspec into the matchspec and comment part.
fn strip_comment(input: &str) -> (&str, Option<&str>) {
    input
        .split_once('#')
        .map(|(spec, comment)| (spec, Some(comment)))
        .unwrap_or_else(|| (input, None))
}

/// Strips any if statements from the matchspec. `if` statements in matchspec are "anticipating
/// future compatibility issues".
fn strip_if(input: &str) -> (&str, Option<&str>) {
    // input
    //     .split_once("if")
    //     .map(|(spec, if_statement)| (spec, Some(if_statement)))
    //     .unwrap_or_else(|| (input, None))
    (input, None)
}

/// Returns true if the specified string represents a package path.
fn is_package_file(input: &str) -> bool {
    input.ends_with(".conda") || input.ends_with(".tar.bz2")
}

/// An optimized data structure to store key value pairs in between a bracket string
/// `[key1=value1, key2=value2]`. The optimization stores two such values on the stack and otherwise
/// allocates a vector on the heap. Two is chosen because that seems to be more than enough for
/// most use cases.
type BracketVec<'a> = SmallVec<[(&'a str, &'a str); 2]>;

/// A parse combinator to filter whitespace if front and after another parser.
fn whitespace_enclosed<'a, F: 'a, O, E: ParseError<&'a str>>(
    inner: F,
) -> impl FnMut(&'a str) -> IResult<&'a str, O, E>
where
    F: FnMut(&'a str) -> IResult<&'a str, O, E>,
{
    delimited(multispace0, inner, multispace0)
}

/// Parses the contents of a bracket list `[version="1,2,3", bla=3]`
fn parse_bracket_list(input: &str) -> Result<BracketVec, ParseMatchSpecError> {
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
                take_while1(|c: char| c.is_alphanumeric() || c == '_' || c == '-'),
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

/// Strips the brackets part of the matchspec returning the rest of the matchspec and  the contents
/// of the brackets as a `Vec<&str>`.
fn strip_brackets(input: &str) -> Result<(Cow<'_, str>, BracketVec), ParseMatchSpecError> {
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

/// Parses a BracketVec into precise components
fn parse_bracket_vec_into_components(
    bracket: BracketVec,
    match_spec: MatchSpec,
) -> Result<MatchSpec, ParseMatchSpecError> {
    let mut match_spec = match_spec.clone();

    for elem in bracket {
        let (key, value) = elem;
        match key {
            "version" => match_spec.version = Some(VersionSpec::from_str(&value)?),
            "build" => match_spec.build = Some(value.to_string()),
            "build_number" => match_spec.build_number = Some(value.parse()?),
            "fn" => match_spec.file_name = Some(value.to_string()),
            _ => Err(ParseMatchSpecError::InvalidBracketKey(key.to_owned()))?,
        }
    }
    Ok(match_spec)
}

/// Strip the package name from the input.
fn strip_package_name(input: &str) -> Result<(&str, &str), ParseMatchSpecError> {
    match take_while1(|c: char| !c.is_whitespace() && !is_start_of_version_constraint(c))(input)
        .finish()
    {
        Ok((input, name)) => Ok((name.trim(), input.trim())),
        Err(nom::error::Error { .. }) => Err(ParseMatchSpecError::MissingPackageName),
    }
}

/// Splits a string into version and build constraints.
fn split_version_and_build(input: &str) -> Result<(&str, Option<&str>), ParseMatchSpecError> {
    fn parse_operator(input: &str) -> IResult<&str, &str> {
        alt((
            tag(">="),
            tag("<="),
            tag("~="),
            tag("=="),
            tag("!="),
            tag("="),
            tag("<"),
            tag(">"),
        ))(input)
    }

    fn parse_constraint(input: &str) -> IResult<&str, &str> {
        recognize(pair(
            whitespace_enclosed(opt(parse_operator)),
            take_till1(|c: char| {
                is_start_of_version_constraint(c)
                    || c.is_whitespace()
                    || matches!(c, ',' | '|' | ')' | '(')
            }),
        ))(input)
    }

    fn parse_version_constraint_or_group(input: &str) -> IResult<&str, &str> {
        alt((
            delimited(tag("("), parse_version_group, tag(")")),
            parse_constraint,
        ))(input)
    }

    fn parse_version_group(input: &str) -> IResult<&str, &str> {
        recognize(separated_list1(
            whitespace_enclosed(one_of(",|")),
            parse_version_constraint_or_group,
        ))(input)
    }

    fn parse_version_and_build_seperator(input: &str) -> IResult<&str, &str> {
        terminated(parse_version_group, opt(one_of(" =")))(input)
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
        Err(nom::error::Error { .. }) => Err(ParseMatchSpecError::InvalidVersionAndBuild),
    }
}

/// Parses a conda match spec.
/// This is based on: https://github.com/conda/conda/blob/master/conda/models/match_spec.py#L569
fn parse(input: &str) -> Result<MatchSpec, ParseMatchSpecError> {
    // Step 1. Strip '#' and `if` statement
    let (input, _comment) = strip_comment(input);
    let (input, _if_clause) = strip_if(input);

    // 2. Is the spec a tarball?
    if is_package_file(input) {
        let _url = match Url::parse(input) {
            Ok(url) => url,
            Err(_) => match PathBuf::from_str(input) {
                Ok(path) => Url::from_file_path(path)
                    .map_err(|_| ParseMatchSpecError::InvalidPackagePathOrUrl)?,
                Err(_) => return Err(ParseMatchSpecError::InvalidPackagePathOrUrl),
            },
        };

        // TODO: Implementing package file specs
        unimplemented!()
    }

    let match_spec = MatchSpec::default();

    // 3. Strip off brackets portion
    let (input, brackets) = strip_brackets(input.trim())?;
    let mut match_spec = parse_bracket_vec_into_components(brackets, match_spec)?;

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

    match_spec.namespace = namespace.map(ToOwned::to_owned).or(match_spec.namespace);

    if let Some(channel_str) = channel_str {
        if let Some((channel, subdir)) = channel_str.rsplit_once("/") {
            match_spec.channel = Some(channel.to_string());
            match_spec.subdir = Some(subdir.to_string());
        } else {
            match_spec.channel = Some(channel_str.to_string());
        }
    }

    // Step 6. Strip off the package name from the input
    let (name, input) = strip_package_name(input)?;
    match_spec.name = Some(name.to_owned());

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

        // Parse the version spec
        match_spec.version = Some(
            VersionSpec::from_str(version_str.as_ref())
                .map_err(ParseMatchSpecError::InvalidVersionSpec)?,
        );

        if let Some(build) = build_str {
            match_spec.build = Some(build.to_owned());
        }
    }

    Ok(match_spec)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{
        split_version_and_build, strip_brackets, BracketVec, MatchSpec, ParseMatchSpecError,
    };
    use crate::{channel, Channel, ChannelConfig, VersionSpec};
    use smallvec::smallvec;

    #[test]
    fn test_strip_brackets() {
        let result = strip_brackets(r#"bla [version="1.2.3"]"#).unwrap();
        assert_eq!(result.0, "bla ");
        let expected: BracketVec = smallvec![("version", "1.2.3")];
        assert_eq!(result.1, expected);

        let result = strip_brackets(r#"bla [version='1.2.3']"#).unwrap();
        assert_eq!(result.0, "bla ");
        let expected: BracketVec = smallvec![("version", "1.2.3")];
        assert_eq!(result.1, expected);

        let result = strip_brackets(r#"bla [version=1]"#).unwrap();
        assert_eq!(result.0, "bla ");
        let expected: BracketVec = smallvec![("version", "1")];
        assert_eq!(result.1, expected);

        let result = strip_brackets(r#"conda-forge::bla[version=1]"#).unwrap();
        assert_eq!(result.0, "conda-forge::bla");
        let expected: BracketVec = smallvec![("version", "1")];
        assert_eq!(result.1, expected);

        let result = strip_brackets(r#"bla [version="1.2.3", build_number=1]"#).unwrap();
        assert_eq!(result.0, "bla ");
        let expected: BracketVec = smallvec![("version", "1.2.3"), ("build_number", "1")];
        assert_eq!(result.1, expected);

        assert_eq!(
            strip_brackets(r#"bla [version="1.2.3", build_number=]"#),
            Err(ParseMatchSpecError::InvalidBracket)
        );
        assert_eq!(
            strip_brackets(r#"bla [version="1.2.3, build_number=1]"#),
            Err(ParseMatchSpecError::InvalidBracket)
        );
    }

    #[test]
    fn test_split_version_and_build() {
        assert_eq!(
            split_version_and_build("=1.2.3 0"),
            Ok(("=1.2.3", Some("0")))
        );
        assert_eq!(split_version_and_build("1.2.3=0"), Ok(("1.2.3", Some("0"))));
        assert_eq!(
            split_version_and_build(">=1.0 , < 2.0 py34_0"),
            Ok((">=1.0 , < 2.0", Some("py34_0")))
        );
        assert_eq!(
            split_version_and_build(">=1.0 , < 2.0 =py34_0"),
            Ok((">=1.0 , < 2.0", Some("=py34_0")))
        );
        assert_eq!(split_version_and_build("=1.2.3 "), Ok(("=1.2.3", None)));
        assert_eq!(
            split_version_and_build(">1.8,<2|==1.7"),
            Ok((">1.8,<2|==1.7", None))
        );
        assert_eq!(
            split_version_and_build("* openblas_0"),
            Ok(("*", Some("openblas_0")))
        );
        assert_eq!(split_version_and_build("* *"), Ok(("*", Some("*"))));
    }

    #[test]
    fn test_match_spec() {
        insta::assert_yaml_snapshot!([
            MatchSpec::from_str("python 3.8.* *_cpython").unwrap(),
            MatchSpec::from_str("foo=1.0=py27_0").unwrap(),
            MatchSpec::from_str("foo==1.0=py27_0").unwrap(),
        ],
        @r###"
        ---
        - name: python
          version: 3.8.*
          build: "*_cpython"
        - name: foo
          version: 1.0.*
          build: py27_0
        - name: foo
          version: "==1.0"
          build: py27_0
        "###);
    }

    #[test]
    fn test_match_spec_more() {
        let spec = MatchSpec::from_str("conda-forge::foo[version=\"1.0.*\"]").unwrap();
        assert_eq!(spec.name, Some("foo".to_string()));
        assert_eq!(spec.version, Some(VersionSpec::from_str("1.0.*").unwrap()));
        assert_eq!(spec.channel, Some("conda-forge".to_string()));
    }
}
