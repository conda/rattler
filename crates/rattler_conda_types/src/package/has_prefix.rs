use crate::package::paths::FileMode;
use nom::{
    branch::alt,
    bytes::complete::{tag, tag_no_case, take_till1},
    character::complete::multispace1,
    combinator::{all_consuming, map, value},
    sequence::{preceded, terminated, tuple},
    IResult,
};
use std::{
    borrow::Cow,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    str::FromStr,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HasPrefixEntry {
    pub prefix: Cow<'static, str>,
    pub file_mode: FileMode,
    pub relative_path: PathBuf,
}

/// Representation of the `info/has_prefix` file in older package archives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HasPrefix {
    pub files: Vec<HasPrefixEntry>,
}

impl HasPrefix {
    /// Parses a `has_prefix` file from a reader.
    pub fn from_reader(mut reader: impl Read) -> Result<Self, std::io::Error> {
        let mut str = String::new();
        reader.read_to_string(&mut str)?;
        Self::from_str(&str)
    }

    /// Parses a `has_prefix` file from a file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        Self::from_reader(File::open(path.as_ref())?)
    }

    /// Reads the file from a package archive directory
    pub fn from_package_directory(path: &Path) -> Result<Self, std::io::Error> {
        Self::from_path(&path.join("info/has_prefix"))
    }
}

impl FromStr for HasPrefix {
    type Err = std::io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            files: s
                .lines()
                .map(HasPrefixEntry::from_str)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl FromStr for HasPrefixEntry {
    type Err = std::io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        return alt((prefix_file_mode_path, only_path))(s)
            .map(|(_, res)| res)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()));

        /// Parses "<prefix> <file_mode> <path>" and fails if there is more input.
        fn prefix_file_mode_path(buf: &str) -> IResult<&str, HasPrefixEntry> {
            all_consuming(map(
                tuple((
                    possibly_quoted_string,
                    multispace1,
                    file_mode,
                    multispace1,
                    possibly_quoted_string,
                )),
                |(prefix, _, file_mode, _, path)| HasPrefixEntry {
                    prefix: Cow::Owned(prefix.into_owned()),
                    file_mode,
                    relative_path: PathBuf::from(path.as_ref()),
                },
            ))(buf)
        }

        /// Parses "<path>" and fails if there is more input.
        fn only_path(buf: &str) -> IResult<&str, HasPrefixEntry> {
            all_consuming(map(possibly_quoted_string, |path| HasPrefixEntry {
                prefix: Cow::Borrowed("/opt/anaconda1anaconda2anaconda3"),
                file_mode: FileMode::Text,
                relative_path: PathBuf::from(path.as_ref()),
            }))(buf)
        }

        /// Parses "text|binary" as a [`FileMode`]
        fn file_mode(buf: &str) -> IResult<&str, FileMode> {
            alt((
                value(FileMode::Text, tag_no_case("text")),
                value(FileMode::Binary, tag_no_case("binary")),
            ))(buf)
        }

        /// Parses either a quoted or an unquoted string.
        fn possibly_quoted_string(buf: &str) -> IResult<&str, Cow<str>> {
            alt((
                map(quoted_string, Cow::Owned),
                map(take_till1(|c: char| c.is_whitespace()), Cow::Borrowed),
            ))(buf)
        }

        /// Parses a quoted string and delimited '\"'
        fn quoted_string(buf: &str) -> IResult<&str, String> {
            let qs = preceded(tag("\""), in_quotes);
            return terminated(qs, tag("\""))(buf);

            fn in_quotes(buf: &str) -> IResult<&str, String> {
                let mut ret = String::new();
                let mut skip_delimiter = false;
                for (i, ch) in buf.char_indices() {
                    if ch == '\\' && !skip_delimiter {
                        skip_delimiter = true;
                    } else if ch == '"' && !skip_delimiter {
                        return Ok((&buf[i..], ret));
                    } else {
                        ret.push(ch);
                        skip_delimiter = false;
                    }
                }
                Err(nom::Err::Incomplete(nom::Needed::Unknown))
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::HasPrefixEntry;
    use crate::package::FileMode;
    use std::{borrow::Cow, path::PathBuf, str::FromStr};

    #[test]
    pub fn test_parse_has_prefix() {
        let parsed =
            HasPrefixEntry::from_str("/opt/anaconda1anaconda2anaconda3 text lib/pkgconfig/zlib.pc")
                .unwrap();
        assert_eq!(
            parsed,
            HasPrefixEntry {
                prefix: Cow::Borrowed("/opt/anaconda1anaconda2anaconda3"),
                file_mode: FileMode::Text,
                relative_path: PathBuf::from("lib/pkgconfig/zlib.pc"),
            }
        );

        let parsed = HasPrefixEntry::from_str(
            "\"/opt/anaconda1 anaconda2anaconda3\" binary \"lib/pkg config/zlib.pc\"",
        )
        .unwrap();
        assert_eq!(
            parsed,
            HasPrefixEntry {
                prefix: Cow::Borrowed("/opt/anaconda1 anaconda2anaconda3"),
                file_mode: FileMode::Binary,
                relative_path: PathBuf::from("lib/pkg config/zlib.pc"),
            }
        );

        let parsed = HasPrefixEntry::from_str("lib/pkgconfig/zlib.pc").unwrap();
        assert_eq!(
            parsed,
            HasPrefixEntry {
                prefix: Cow::Borrowed("/opt/anaconda1anaconda2anaconda3"),
                file_mode: FileMode::Text,
                relative_path: PathBuf::from("lib/pkgconfig/zlib.pc"),
            }
        );
    }
}
