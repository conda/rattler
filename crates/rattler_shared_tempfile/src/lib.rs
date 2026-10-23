#![deny(missing_docs)]

//! Create temporary files that are renamed into a directory that may be
//! shared with other users, such as a cache directory.

/// Returns a [`tempfile::Builder`] for temporary files that are renamed into a
/// directory that may be shared with other users, such as a cache directory.
///
/// `tempfile` creates files that are only readable by the owner (mode `0600`),
/// which neither the umask nor a default ACL can widen. Files created by this
/// builder start out with mode `0666` instead, like `File::create` does, so
/// the umask or a default ACL of the directory decides who else can read
/// them.
pub fn builder<'a, 'b>() -> tempfile::Builder<'a, 'b> {
    #[allow(unused_mut)]
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    builder.permissions(std::os::unix::fs::PermissionsExt::from_mode(0o666));
    builder
}
