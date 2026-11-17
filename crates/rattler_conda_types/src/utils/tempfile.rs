//! Temporary files for atomic writes of shared data.

use std::{io, path::Path};

use tempfile::{Builder, NamedTempFile};

/// Returns a [`Builder`] whose files get the permissions a plain
/// `File::create` would give them (`0o666` minus the umask), instead of
/// `tempfile`'s default `0o600`.
///
/// Use it for files that are written to a temporary file and then persisted
/// into place, and that other users may need to read, such as prefix records
/// in `conda-meta` and shared caches. Never use it for files holding secrets.
pub fn shared_tempfile_builder<'a, 'b>() -> Builder<'a, 'b> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // This is the mode passed to `open(2)`, which clears the umask bits
        // from it, just like `File::create` does with its default `0o666`.
        builder.permissions(std::fs::Permissions::from_mode(0o666));
    }
    builder
}

/// Creates a temporary file in `dir`, see [`shared_tempfile_builder`].
pub fn shared_tempfile_in(dir: impl AsRef<Path>) -> io::Result<NamedTempFile> {
    shared_tempfile_builder().tempfile_in(dir)
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::shared_tempfile_in;

    fn mode(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn persisted_file_has_same_mode_as_file_create() {
        let dir = tempfile::tempdir().unwrap();
        let reference = dir.path().join("reference");
        std::fs::File::create(&reference).unwrap();

        let target = dir.path().join("target");
        shared_tempfile_in(dir.path())
            .unwrap()
            .persist(&target)
            .unwrap();

        assert_eq!(mode(&target), mode(&reference));
    }
}
