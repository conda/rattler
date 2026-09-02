//! Functions that enable extracting or streaming a Conda package for objects that implement the
//! [`std::io::Read`] trait.

use super::{ExtractError, ExtractResult};
use std::io::{Seek, SeekFrom, copy};
use std::mem::ManuallyDrop;
use std::{
    collections::HashSet,
    ffi::OsStr,
    io::Read,
    path::{Component, Path, PathBuf},
};
use tempfile::SpooledTempFile;
use zip::read::{ZipArchive, ZipFile, read_zipfile_from_stream};

/// The minimum safe timestamp (1980-01-01T00:00:00 UTC) for filesystems like exFAT
/// that do not support timestamps before 1980.
const SAFE_MTIME_FLOOR: u64 = 315_532_800;

/// Returns the `.tar.bz2` as a decompressed `tar::Archive`. The `tar::Archive` can be used to
/// extract the files from it, or perform introspection.
pub fn stream_tar_bz2(reader: impl Read) -> tar::Archive<impl Read + Sized> {
    tar::Archive::new(bzip2::read::BzDecoder::new(reader))
}

/// Returns the `.tar.zst` as a decompressed `tar` archive. The `tar::Archive` can be used to
/// extract the files from it, or perform introspection.
pub(crate) fn stream_tar_zst(
    reader: impl Read,
) -> Result<tar::Archive<impl Read + Sized>, ExtractError> {
    Ok(tar::Archive::new(zstd::stream::read::Decoder::new(reader)?))
}

/// Extracts the contents a `.tar.bz2` package archive.
pub fn extract_tar_bz2(
    reader: impl Read,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;

    process_with_hashing(reader, |reader| {
        let mut archive = stream_tar_bz2(reader);
        unpack_tar_archive_sync(&mut archive, destination)?;
        drain_decoder(archive.into_inner())?;
        Ok(())
    })
}

/// Reads a decompressor to its end after the tar reader stopped at the
/// end-of-archive marker, so a stream cut short in its trailer is reported
/// instead of accepted.
fn drain_decoder(mut decoder: impl Read) -> Result<(), ExtractError> {
    copy(&mut decoder, &mut std::io::sink())?;
    Ok(())
}

/// Extracts the contents of a `.conda` package archive.
pub fn extract_conda_via_streaming(
    reader: impl Read,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    // Construct the destination path if it doesn't exist yet
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;

    process_with_hashing(reader, |reader| {
        while let Some(file) = read_zipfile_from_stream(reader)? {
            extract_zipfile(file, destination)?;
        }
        Ok(())
    })
}

/// Extracts the contents of a .conda package archive by fully reading the stream and then decompressing
pub fn extract_conda_via_buffering(
    reader: impl Read,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    // delete destination first, as this method is usually used as a fallback from a failed streaming decompression
    if destination.exists() {
        std::fs::remove_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;
    }
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;

    process_with_hashing(reader, |reader| {
        // Create a SpooledTempFile with a 5MB limit
        let mut temp_file = SpooledTempFile::new(5 * 1024 * 1024);
        copy(reader, &mut temp_file)?;
        temp_file.seek(SeekFrom::Start(0))?;
        let mut archive = ZipArchive::new(temp_file)?;

        for i in 0..archive.len() {
            let file = archive.by_index(i)?;
            extract_zipfile(file, destination)?;
        }
        Ok(())
    })
}

fn extract_zipfile<R: std::io::Read>(
    zip_file: ZipFile<'_, R>,
    destination: &Path,
) -> Result<(), ExtractError> {
    // If an error occurs while we are reading the contents of the zip we don't want to
    // seek to the end of the file. Using [`ManuallyDrop`] we prevent `drop` to be called on
    // the `file` in case the stack unwinds.
    let mut file = ManuallyDrop::new(zip_file);

    if file
        .mangled_name()
        .file_name()
        .map(OsStr::to_string_lossy)
        .is_some_and(|file_name| file_name.ends_with(".tar.zst"))
    {
        let mut archive = stream_tar_zst(&mut *file)?;
        unpack_tar_archive_sync(&mut archive, destination)?;
        drain_decoder(archive.into_inner())?;
    } else {
        // Manually read to the end of the stream if that didn't happen.
        std::io::copy(&mut *file, &mut std::io::sink())?;
    }

    // Take the file out of the [`ManuallyDrop`] to properly drop it.
    let _ = ManuallyDrop::into_inner(file);

    Ok(())
}

/// Unpacks a tar archive into `destination`.
///
/// Entry paths are sanitized like [`tar::Entry::unpack_in`] does, but each
/// parent directory is validated against the destination only the first time
/// it is seen. `unpack_in` canonicalizes both the parent and the destination
/// for every entry, which dominates extraction time on Windows.
///
/// Mtimes are set manually with clamping (to `SAFE_MTIME_FLOOR`) and error
/// handling, so filesystems like exFAT that cannot represent timestamps
/// before 1980-01-01 do not fail the extraction.
fn unpack_tar_archive_sync<R: Read>(
    archive: &mut tar::Archive<R>,
    destination: &Path,
) -> Result<(), ExtractError> {
    archive.set_preserve_mtime(false);

    // `dunce` keeps Windows paths in their normal form instead of the `\\?\`
    // verbatim form that `std::fs::canonicalize` returns.
    let destination = dunce::canonicalize(destination).map_err(ExtractError::IoError)?;
    let mut validated_parents: HashSet<PathBuf> = HashSet::new();

    for entry in archive.entries().map_err(ExtractError::IoError)? {
        let mut entry = entry.map_err(ExtractError::IoError)?;
        let entry_type = entry.header().entry_type();
        let mtime = entry.header().mtime().unwrap_or(0);
        let entry_path = entry.path().map_err(ExtractError::IoError)?.into_owned();
        // Old tar formats mark directories with a trailing slash only.
        let is_regular_file = entry_type.is_file()
            && !(entry.header().as_ustar().is_none() && entry.path_bytes().ends_with(b"/"));

        // Entries with `..` components or that resolve to the destination
        // itself are skipped, like `unpack_in` does.
        let Some(file_dst) = unpacked_destination_path(&destination, &entry_path) else {
            continue;
        };
        let Some(parent) = file_dst.parent() else {
            continue;
        };

        if cfg!(windows) && entry_type.is_symlink() {
            // Creating symlinks requires elevated privileges or developer
            // mode on Windows. Packages that ship them still extract, minus
            // the links.
            tracing::warn!("Skipping symlink in tar archive: {}", entry_path.display());
            continue;
        }

        // A link can redirect a directory that was validated earlier, so
        // every parent is validated again after one.
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            validated_parents.clear();
        }

        if !validated_parents.contains(parent) {
            ensure_dir_inside(&destination, parent)
                .map_err(|err| unpack_error(&entry_path, err))?;
            validated_parents.insert(parent.to_path_buf());
        }

        if is_regular_file {
            unpack_file(&mut entry, &file_dst, mtime)
                .map_err(|err| unpack_error(&entry_path, err))?;
        } else {
            if entry_type.is_hard_link() {
                unpack_hard_link(&destination, &entry, &file_dst)
            } else {
                entry.unpack(&file_dst).map(|_| ())
            }
            .map_err(|err| unpack_error(&entry_path, err))?;
            set_mtime_safe(&file_dst, mtime);
        }
    }

    Ok(())
}

/// Names the archive entry in an error from unpacking it.
fn unpack_error(entry_path: &Path, err: std::io::Error) -> ExtractError {
    ExtractError::IoError(std::io::Error::new(
        err.kind(),
        format!("failed to unpack `{}`: {err}", entry_path.display()),
    ))
}

/// Largest write buffer used when unpacking a regular file.
const UNPACK_WRITE_BUFFER_SIZE: usize = 128 * 1024;

/// Writes a regular file entry to `file_dst`.
///
/// [`tar::Entry::unpack`] copies with 8 KiB writes; buffering up to the file
/// size cuts the number of write calls, which dominates on Windows. The mtime
/// is set through the open handle, saving a reopen per file.
fn unpack_file<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    file_dst: &Path,
    mtime: u64,
) -> std::io::Result<()> {
    fn create_new(path: &Path) -> std::io::Result<std::fs::File> {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
    }

    // Always write a new file instead of truncating in place, so an existing
    // hard link or symlink at the destination cannot redirect the write.
    let file = match create_new(file_dst) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(file_dst)?;
            create_new(file_dst)?
        }
        Err(err) => return Err(err),
    };

    let capacity = usize::try_from(entry.size())
        .unwrap_or(UNPACK_WRITE_BUFFER_SIZE)
        .min(UNPACK_WRITE_BUFFER_SIZE);
    let mut writer = std::io::BufWriter::with_capacity(capacity, file);
    copy(entry, &mut writer)?;
    let file = writer
        .into_inner()
        .map_err(std::io::IntoInnerError::into_error)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(mode) = entry.header().mode() {
            file.set_permissions(std::fs::Permissions::from_mode(mode & 0o777))?;
        }
    }

    let clamped = std::cmp::max(mtime, SAFE_MTIME_FLOOR);
    let file_time = filetime::FileTime::from_unix_time(clamped as i64, 0);
    if let Err(e) = filetime::set_file_handle_times(&file, None, Some(file_time)) {
        tracing::warn!(
            "Failed to set mtime for '{}': {}. \
             The target filesystem may not support this timestamp. \
             This does not affect package integrity.",
            file_dst.display(),
            e
        );
    }

    Ok(())
}

/// Creates `dir` and any missing ancestors, after checking that the deepest
/// existing ancestor resolves inside `destination`. Checking before creating
/// keeps a symlinked ancestor from leading directory creation outside the
/// destination.
fn ensure_dir_inside(destination: &Path, dir: &Path) -> std::io::Result<()> {
    let mut missing = Vec::new();
    let mut ancestor = dir;
    while ancestor.symlink_metadata().is_err() {
        missing.push(ancestor);
        match ancestor.parent() {
            Some(parent) => ancestor = parent,
            None => break,
        }
    }

    validate_inside(destination, ancestor)?;
    for dir in missing.into_iter().rev() {
        match std::fs::create_dir(dir) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// Errors when the canonical form of `path` is not inside `destination`.
fn validate_inside(destination: &Path, path: &Path) -> std::io::Result<()> {
    let canonical = dunce::canonicalize(path)?;
    if canonical.starts_with(destination) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "trying to unpack outside of destination path: {}",
                path.display()
            ),
        ))
    }
}

/// Creates the hard link described by `entry` at `file_dst`. The link source
/// is resolved relative to the destination and must already exist inside it.
fn unpack_hard_link<R: Read>(
    destination: &Path,
    entry: &tar::Entry<'_, R>,
    file_dst: &Path,
) -> std::io::Result<()> {
    let link_name = entry.link_name()?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "hard link entry without a link name",
        )
    })?;
    let link_src = unpacked_destination_path(destination, &link_name).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "hard link source escapes the destination: {}",
                link_name.display()
            ),
        )
    })?;
    validate_inside(destination, &link_src)?;
    match std::fs::hard_link(&link_src, file_dst) {
        // A retried extraction into the same destination finds the link from
        // the previous attempt.
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(file_dst)?;
            std::fs::hard_link(&link_src, file_dst)
        }
        result => result,
    }
}

/// Resolves the on-disk path a tar entry is unpacked to, mirroring the
/// sanitization in [`tar::Entry::unpack_in`]: absolute-path roots and `.`
/// components are stripped and a `..` component makes the entry unsafe.
///
/// Returns `None` when the entry would not map to a distinct path inside
/// `destination`, so callers never set metadata on a path that could resolve
/// outside the extraction directory.
fn unpacked_destination_path(destination: &Path, entry_path: &Path) -> Option<PathBuf> {
    let mut full_path = destination.to_path_buf();
    for component in entry_path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
            Component::ParentDir => return None,
            Component::Normal(part) => full_path.push(part),
        }
    }

    if full_path == destination {
        return None;
    }

    Some(full_path)
}

/// Sets the modification time of `path` itself, without following a symlink,
/// clamping to a safe minimum and logging a warning on failure instead of
/// propagating the error. Never following keeps a symlink entry from
/// redirecting the write to a location outside the destination.
fn set_mtime_safe(path: &Path, mtime: u64) {
    let clamped = std::cmp::max(mtime, SAFE_MTIME_FLOOR);
    let file_time = filetime::FileTime::from_unix_time(clamped as i64, 0);

    if let Err(e) = filetime::set_symlink_file_times(path, file_time, file_time) {
        tracing::warn!(
            "Failed to set mtime for '{}': {}. \
             The target filesystem may not support this timestamp. \
             This does not affect package integrity.",
            path.display(),
            e
        );
    }
}

// Define a custom reader to track file size
pub(crate) struct SizeCountingReader<R> {
    inner: R,
    size: u64,
}

impl<R> SizeCountingReader<R> {
    pub(crate) fn new(inner: R) -> Self {
        Self { inner, size: 0 }
    }

    pub(crate) fn finalize(self) -> (R, u64) {
        (self.inner, self.size)
    }
}

impl<R: Read> Read for SizeCountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let bytes_read = self.inner.read(buf)?;
        self.size += bytes_read as u64;
        Ok(bytes_read)
    }
}

// AsyncRead implementation for use with tokio
impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for SizeCountingReader<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let previously_filled = buf.filled().len();

        // Since R: Unpin, we can safely use get_mut
        let this = self.as_mut().get_mut();
        let reader = std::pin::Pin::new(&mut this.inner);

        match reader.poll_read(cx, buf) {
            std::task::Poll::Ready(Ok(())) => {
                let bytes_read = buf.filled().len() - previously_filled;
                this.size += bytes_read as u64;
                std::task::Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

/// Helper function to compute hashes and size while processing a tar archive
fn process_with_hashing<E, R, F>(reader: R, processor: F) -> Result<ExtractResult, E>
where
    R: Read,
    E: From<std::io::Error>,
    F: FnOnce(
        &mut SizeCountingReader<
            &mut rattler_digest::HashingReader<
                rattler_digest::HashingReader<R, rattler_digest::Sha256>,
                rattler_digest::Md5,
            >,
        >,
    ) -> Result<(), E>,
{
    // Wrap the reading in additional readers that will compute the hashes of the file while its
    // being read, and count the total size.
    let sha256_reader = rattler_digest::HashingReader::<_, rattler_digest::Sha256>::new(reader);
    let mut md5_reader =
        rattler_digest::HashingReader::<_, rattler_digest::Md5>::new(sha256_reader);
    let mut size_reader = SizeCountingReader::new(&mut md5_reader);

    processor(&mut size_reader)?;

    // Read the file to the end to make sure the hash is properly computed
    std::io::copy(&mut size_reader, &mut std::io::sink())?;

    // Get the size and hashes
    let (_, total_size) = size_reader.finalize();

    // An empty stream decodes as an empty archive without any decoder
    // complaining, so reject it here.
    if total_size == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "no data was read from the package stream - the stream may have been truncated",
        )
        .into());
    }
    let (sha256_reader, md5) = md5_reader.finalize();
    let (_, sha256) = sha256_reader.finalize();

    Ok(ExtractResult {
        sha256,
        md5,
        total_size,
    })
}
