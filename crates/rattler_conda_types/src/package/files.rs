use std::{
    path::{Path, PathBuf},
    io::Read,
    fs::File,
    str::FromStr
};

/// Representation of the `info/files` file in older package archives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Files {
    pub files: Vec<PathBuf>,
}

impl Files {
    /// Parses a `files` file from a reader.
    pub fn from_reader(mut reader: impl Read) -> Result<Self, std::io::Error> {
        let mut str = String::new();
        reader.read_to_string(&mut str)?;
        Self::from_str(&str)
    }

    /// Parses a `files` file from a file.
    pub fn from_path(path: &Path) -> Result<Self, std::io::Error> {
        Self::from_reader(File::open(path)?)
    }

    /// Reads the file from a package archive directory
    pub fn from_package_directory(path: &Path) -> Result<Self, std::io::Error> {
        Self::from_path(&path.join("info/files"))
    }
}

impl FromStr for Files {
    type Err = std::io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            files: s.lines().map(PathBuf::from).collect(),
        })
    }
}

#[cfg(test)]
mod test {
    use super::Files;
    use std::{path::PathBuf, str::FromStr};

    #[test]
    pub fn test_parse_files() {
        let parsed = Files::from_str("include/zconf.h\ninclude/zlib.h\nlib/libz.a\nlib/libz.so\nlib/libz.so.1\nlib/libz.so.1.2.8\nlib/pkgconfig/zlib.pc").unwrap();
        assert_eq!(
            parsed,
            Files {
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
