#[cfg(test)]
mod tests {
    use crate::metadata::FSMetadata;
    use crate::virtual_fs_core::*;
    use rattler_conda_types::package::PathType;
    use std::{
        ffi::{OsStr, OsString},
        path::{Path, PathBuf},
        sync::Arc,
    };
    use tempfile::tempdir;

    fn create_test_fs() -> (VirtualFSCore, tempfile::TempDir) {
        let tmp = tempdir().unwrap();

        let cache_root: Arc<Path> = Arc::from(tmp.path());

        std::fs::create_dir_all(tmp.path().join("bin")).unwrap();
        std::fs::write(tmp.path().join("bin").join("hello.txt"), b"hello world").unwrap();

        let mut metadata = vec![
            // index 0 → inode 1 (root / "bin" directory)
            FSMetadata::new_directory(PathBuf::from("bin"), 0),
            // index 1 → inode 2 (file)
            FSMetadata::new_file(
                OsString::from("hello.txt"),
                0,
                cache_root,
                PathType::HardLink,
                None,
            ),
        ];

        metadata[0].as_directory_mut().unwrap().children.push(1);

        let fs = VirtualFSCore::new(metadata, PathBuf::from("/mounted"));

        (fs, tmp)
    }

    #[test]
    fn lookup_existing_file() {
        let (fs, _) = create_test_fs();

        // parent ino=1 (root), returns 0-based child index=1
        assert_eq!(fs.lookup(1, OsStr::new("hello.txt")), Some(1));
    }

    #[test]
    fn lookup_missing_file() {
        let (fs, _) = create_test_fs();

        assert_eq!(fs.lookup(1, OsStr::new("missing.txt")), None);
    }

    #[test]
    fn getattr_directory() {
        let (fs, _) = create_test_fs();

        let attr = fs.getattr(1).unwrap(); // ino=1 → index 0 → directory

        assert!(attr.is_dir);
        assert_eq!(attr.size, 0);
        assert_eq!(attr.perm, 0o755);
    }

    #[test]
    fn getattr_file() {
        let (fs, _tmp) = create_test_fs();

        let attr = fs.getattr(2).unwrap(); // ino=2 → index 1 → file

        assert!(!attr.is_dir);
        assert_eq!(attr.size, 11);
    }

    #[test]
    fn open_and_read() {
        let (fs, _tmp) = create_test_fs();

        let fh = fs.open(2).unwrap(); // ino=2 → file

        let bytes = fs.read(2, fh, 0, 5).unwrap();

        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn read_middle_of_file() {
        let (fs, _tmp) = create_test_fs();

        let fh = fs.open(2).unwrap();

        let bytes = fs.read(2, fh, 6, 5).unwrap();

        assert_eq!(bytes, b"world");
    }

    #[test]
    fn read_past_end_returns_empty() {
        let (fs, _tmp) = create_test_fs();

        let fh = fs.open(2).unwrap();

        let bytes = fs.read(2, fh, 100, 10).unwrap();

        assert!(bytes.is_empty());
    }

    #[test]
    fn open_is_cached() {
        let (fs, _tmp) = create_test_fs();

        let fh1 = fs.open(2).unwrap();
        let fh2 = fs.open(2).unwrap();

        assert_eq!(fh1, fh2);
    }

    #[test]
    fn release_invalidates_handle() {
        let (fs, _tmp) = create_test_fs();

        let fh = fs.open(2).unwrap();

        fs.release(fh);

        assert!(fs.read(2, fh, 0, 5).is_err());
    }

    #[test]
    fn reopen_after_release_gets_new_handle() {
        let (fs, _tmp) = create_test_fs();

        let fh1 = fs.open(2).unwrap();

        fs.release(fh1);

        let fh2 = fs.open(2).unwrap();

        assert_ne!(fh1, fh2);
    }

    #[test]
    fn readdir_lists_file() {
        let (fs, _) = create_test_fs();

        let entries = fs.readdir(1).unwrap(); // ino=1 → root directory

        assert_eq!(entries.len(), 1);

        let entry = &entries[0];

        assert_eq!(entry.name, OsStr::new("hello.txt"));
        assert!(!entry.is_dir);

        // metadata index 1 → inode 2
        assert_eq!(entry.ino, 2);
    }

    #[test]
    fn invalid_inode_errors() {
        let (fs, _) = create_test_fs();

        assert!(fs.getattr(0).is_err()); // ino=0 is never valid
        assert!(fs.getattr(100).is_err()); // out of range
        assert!(fs.open(100).is_err());
    }

    #[test]
    fn invalid_handle_errors() {
        let (fs, _) = create_test_fs();

        assert!(fs.read(2, 999, 0, 10).is_err());
    }
}
