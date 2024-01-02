use crate::package::PackageFile;
use std::path::{Path, PathBuf};

/// Representation of the `info/files` file in older package archives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Files {
    /// A list of files in the package.
    pub files: Vec<PathBuf>,
}

impl PackageFile for Files {
    fn package_path() -> &'static Path {
        Path::new("info/files")
    }

    fn from_str(str: &str) -> Result<Self, std::io::Error> {
        Ok(Self {
            files: str.lines().map(PathBuf::from).collect(),
        })
    }
}

#[cfg(test)]
mod test {
    use super::{Files, PackageFile};
    use std::path::PathBuf;

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
        );
    }
}
