use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    str::FromStr,
};

/// Representation of the `info/no_softlink` file in older package archives. This file contains a list
/// of all files that should not be "softlinked".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoSoftlink {
    pub files: Vec<PathBuf>,
}

impl NoSoftlink {
    /// Parses a `no_softlink` file from a reader.
    pub fn from_reader(mut reader: impl Read) -> Result<Self, std::io::Error> {
        let mut str = String::new();
        reader.read_to_string(&mut str)?;
        Self::from_str(&str)
    }

    /// Parses a `no_softlink` file from a file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        Self::from_reader(File::open(path.as_ref())?)
    }

    /// Reads the file from a package archive directory
    pub fn from_package_directory(path: &Path) -> Result<Self, std::io::Error> {
        Self::from_path(&path.join("info/no_softlink"))
    }
}

impl FromStr for NoSoftlink {
    type Err = std::io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            files: s.lines().map(PathBuf::from).collect(),
        })
    }
}

#[cfg(test)]
mod test {
    use super::NoSoftlink;
    use std::{path::PathBuf, str::FromStr};

    #[test]
    pub fn test_parse_no_link() {
        let parsed = NoSoftlink::from_str("include/zconf.h\ninclude/zlib.h\nlib/libz.a\nlib/libz.so\nlib/libz.so.1\nlib/libz.so.1.2.8\nlib/pkgconfig/zlib.pc").unwrap();
        assert_eq!(
            parsed,
            NoSoftlink {
                files: vec![
                    PathBuf::from("include/zconf.h"),
                    PathBuf::from("include/zlib.h"),
                    PathBuf::from("lib/libz.a"),
                    PathBuf::from("lib/libz.so"),
                    PathBuf::from("lib/libz.so.1"),
                    PathBuf::from("lib/libz.so.1.2.8"),
                    PathBuf::from("lib/pkgconfig/zlib.pc"),
                ]
            }
        )
    }
}
