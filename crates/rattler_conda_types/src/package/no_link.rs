use super::PackageFile;
use std::path::{Path, PathBuf};

/// Representation of the `info/no_link` file in older package archives. This file contains a list
/// of all files that should not be "linked" (i.e. hard linked) but copied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoLink {
    /// A list of files in the package that should not be "linked" (i.e. hard linked) but copied.
    pub files: Vec<PathBuf>,
}

impl PackageFile for NoLink {
    fn package_path() -> &'static Path {
        Path::new("info/no_link")
    }

    fn from_str(str: &str) -> Result<Self, std::io::Error> {
        Ok(Self {
            files: str.lines().map(PathBuf::from).collect(),
        })
    }
}

#[cfg(test)]
mod test {
    use super::{NoLink, PackageFile};
    use std::path::PathBuf;

    #[test]
    pub fn test_parse_no_link() {
        let parsed = NoLink::from_str("include/zconf.h\ninclude/zlib.h\nlib/libz.a\nlib/libz.so\nlib/libz.so.1\nlib/libz.so.1.2.8\nlib/pkgconfig/zlib.pc").unwrap();
        assert_eq!(
            parsed,
            NoLink {
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
        );
    }
}
