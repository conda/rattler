use crate::package::PackageFile;
use std::path::{Path, PathBuf};

/// Representation of the `info/no_softlink` file in older package archives. This file contains a list
/// of all files that should not be "softlinked".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoSoftlink {
    /// A list of files in the package that should not be "softlinked".
    pub files: Vec<PathBuf>,
}

impl PackageFile for NoSoftlink {
    fn package_path() -> &'static Path {
        Path::new("info/no_softlink")
    }

    fn from_str(str: &str) -> Result<Self, std::io::Error> {
        Ok(Self {
            files: str.lines().map(PathBuf::from).collect(),
        })
    }
}

#[cfg(test)]
mod test {
    use super::{NoSoftlink, PackageFile};
    use std::path::PathBuf;

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
        );
    }
}
