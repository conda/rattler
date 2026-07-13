use nom::{
    IResult, Parser,
    branch::alt,
    bytes::complete::{tag, take_while},
    character::complete::{alpha1, digit1, multispace0, u32},
    combinator::{cut, not, opt, recognize, value},
    error::{ContextError, ParseError, context},
    sequence::{delimited, preceded, terminated},
};
use thiserror::Error;

use crate::version_spec::{EqualityOperator, RangeOperator, StrictRangeOperator, VersionOperators};

#[derive(Debug, Clone, Error, Eq, PartialEq)]
pub enum ParseVersionTreeError {
    #[error("{0}")]
    ParseError(String),
}

/// A parser that parses version operators.
fn parse_operator<'a, E: ParseError<&'a str>>(
    input: &'a str,
) -> Result<(&'a str, VersionOperators), nom::Err<E>> {
    alt((
        value(VersionOperators::Exact(EqualityOperator::Equals), tag("==")),
        value(
            VersionOperators::Exact(EqualityOperator::NotEquals),
            tag("!="),
        ),
        value(
            VersionOperators::StrictRange(StrictRangeOperator::StartsWith),
            tag("="),
        ),
        value(
            VersionOperators::Range(RangeOperator::GreaterEquals),
            tag(">="),
        ),
        value(VersionOperators::Range(RangeOperator::Greater), tag(">")),
        value(
            VersionOperators::Range(RangeOperator::LessEquals),
            tag("<="),
        ),
        value(VersionOperators::Range(RangeOperator::Less), tag("<")),
        value(
            VersionOperators::StrictRange(StrictRangeOperator::Compatible),
            tag("~="),
        ),
    ))
    .parse(input)
}

/// Recognizes the version epoch
fn parse_version_epoch<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> Result<(&'a str, u32), nom::Err<E>> {
    terminated(u32, tag("!")).parse(input)
}

/// A parser that recognizes a version
pub(crate) fn recognize_version<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    allow_glob: bool,
) -> impl FnMut(&'a str) -> IResult<&'a str, &'a str, E> {
    /// Recognizes a single version component (`1`, `a`, `alpha`, `grub`)
    fn recognize_version_component<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
        allow_glob: bool,
    ) -> impl FnMut(&'a str) -> IResult<&'a str, &'a str, E> {
        move |input: &'a str| {
            let ident = alpha1;
            let num = digit1;
            let glob = tag("*");
            if allow_glob {
                alt((ident, num, glob)).parse(input)
            } else {
                alt((ident, num)).parse(input)
            }
        }
    }

    /// Recognize one or more version components (`1.2.3`)
    fn recognize_version_components<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
        allow_glob: bool,
    ) -> impl FnMut(&'a str) -> IResult<&'a str, &'a str, E> {
        move |input: &'a str| {
            // Parse the first component (required)
            let (mut rest, _) = recognize_version_component::<E>(allow_glob)(input)?;
            // Parse subsequent separator + component pairs without allocating
            loop {
                let after_sep = match opt(take_while::<_, _, E>(|c: char| {
                    c == '.' || c == '-' || c == '_'
                }))
                .parse(rest)
                {
                    Ok((r, _)) => r,
                    Err(_) => break,
                };
                match recognize_version_component::<E>(allow_glob)(after_sep) {
                    Ok((r, _)) => rest = r,
                    Err(_) => break,
                }
            }
            let matched = &input[..input.len() - rest.len()];
            Ok((rest, matched))
        }
    }

    move |input: &'a str| {
        recognize((
            // Optional version epoch
            opt(context("epoch", parse_version_epoch)),
            // Version components
            context("components", recognize_version_components(allow_glob)),
            // Local version
            opt(preceded(
                tag("+"),
                cut(context("local", recognize_version_components(allow_glob))),
            )),
        ))
        .parse(input)
    }
}

/// Recognize a version followed by a .* or *, or just a *
pub(crate) fn recognize_version_with_star<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> Result<(&'a str, &'a str), nom::Err<E>> {
    alt((
        // A version with an optional * or .*.
        terminated(
            recognize_version(true),
            take_while(|c: char| c == '.' || c == '*'),
        ),
        // Just a *
        tag("*"),
    ))
    .parse(input)
}

/// A parser that recognized a constraint but does not actually parse it.
pub(crate) fn recognize_constraint<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> Result<(&'a str, &'a str), nom::Err<E>> {
    alt((
        // Any (* or *.*)
        terminated(tag("*"), cut(opt(tag(".*")))),
        // Regex
        recognize(delimited(opt(tag("^")), not(tag("$")), tag("$"))),
        // Version with optional operator followed by optional glob.
        recognize(preceded(
            opt(delimited(
                opt(multispace0),
                parse_operator,
                opt(multispace0),
            )),
            cut(context("version", recognize_version_with_star)),
        )),
    ))
    .parse(input)
}

#[cfg(test)]
mod tests {
    use super::{parse_operator, recognize_version};
    use crate::version_spec::{
        EqualityOperator, RangeOperator, StrictRangeOperator, VersionOperators,
        version_tree::{parse_version_epoch, recognize_constraint},
    };

    #[test]
    fn test_parse_operator() {
        type Err<'a> = nom::error::Error<&'a str>;

        assert_eq!(
            parse_operator::<Err<'_>>("=="),
            Ok(("", VersionOperators::Exact(EqualityOperator::Equals)))
        );
        assert_eq!(
            parse_operator::<Err<'_>>("!="),
            Ok(("", VersionOperators::Exact(EqualityOperator::NotEquals)))
        );
        assert_eq!(
            parse_operator::<Err<'_>>(">"),
            Ok(("", VersionOperators::Range(RangeOperator::Greater)))
        );
        assert_eq!(
            parse_operator::<Err<'_>>(">="),
            Ok(("", VersionOperators::Range(RangeOperator::GreaterEquals)))
        );
        assert_eq!(
            parse_operator::<Err<'_>>("<"),
            Ok(("", VersionOperators::Range(RangeOperator::Less)))
        );
        assert_eq!(
            parse_operator::<Err<'_>>("<="),
            Ok(("", VersionOperators::Range(RangeOperator::LessEquals)))
        );
        assert_eq!(
            parse_operator::<Err<'_>>("="),
            Ok((
                "",
                VersionOperators::StrictRange(StrictRangeOperator::StartsWith)
            ))
        );
        assert_eq!(
            parse_operator::<Err<'_>>("~="),
            Ok((
                "",
                VersionOperators::StrictRange(StrictRangeOperator::Compatible)
            ))
        );

        // Anything else is an error
        assert!(parse_operator::<Err<'_>>("").is_err());
        assert!(parse_operator::<Err<'_>>("  >=").is_err());

        // Only the operator is parsed
        assert_eq!(
            parse_operator::<Err<'_>>(">=3.8"),
            Ok(("3.8", VersionOperators::Range(RangeOperator::GreaterEquals)))
        );
    }

    #[test]
    fn test_recognize_version() {
        type Err<'a> = nom::error::Error<&'a str>;

        assert_eq!(
            recognize_version::<Err<'_>>(false)("3.8.9"),
            Ok(("", "3.8.9"))
        );
        assert_eq!(recognize_version::<Err<'_>>(false)("3"), Ok(("", "3")));
        assert_eq!(
            recognize_version::<Err<'_>>(false)("1!3.8.9+3.4-alpha.2"),
            Ok(("", "1!3.8.9+3.4-alpha.2"))
        );
        assert_eq!(recognize_version::<Err<'_>>(false)("3."), Ok((".", "3")));
        assert_eq!(recognize_version::<Err<'_>>(false)("3.*"), Ok((".*", "3")));

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

        for version_str in versions {
            assert_eq!(
                recognize_version::<Err<'_>>(false)(version_str),
                Ok(("", version_str))
            );
        }
    }

    #[test]
    fn test_parse_version_epoch() {
        type Err<'a> = nom::error::Error<&'a str>;

        assert_eq!(
            parse_version_epoch::<Err<'_>>("1!1.0b2.post345.dev456"),
            Ok(("1.0b2.post345.dev456", 1))
        );

        // Epochs must be integers
        assert!(
            parse_version_epoch::<Err<'_>>("12.23!1").is_err(),
            "epochs should only be integers"
        );
    }

    #[test]
    fn test_recognize_constraint() {
        type Err<'a> = nom::error::Error<&'a str>;

        assert_eq!(recognize_constraint::<Err<'_>>("*"), Ok(("", "*")));
        assert_eq!(recognize_constraint::<Err<'_>>("3.8"), Ok(("", "3.8")));
        assert_eq!(recognize_constraint::<Err<'_>>("3.8*"), Ok(("", "3.8*")));
        assert_eq!(recognize_constraint::<Err<'_>>("3.8.*"), Ok(("", "3.8.*")));
        assert_eq!(
            recognize_constraint::<Err<'_>>(">=3.8.*"),
            Ok(("", ">=3.8.*"))
        );
        assert_eq!(
            recognize_constraint::<Err<'_>>(">=3.8.*<3.9"),
            Ok(("<3.9", ">=3.8.*"))
        );
        assert_eq!(
            recognize_constraint::<Err<'_>>(">=3.8.*,<3.9"),
            Ok((",<3.9", ">=3.8.*"))
        );
    }
}
