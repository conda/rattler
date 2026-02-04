//! Synchronous writer for the Content Addressable Store.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use rattler_digest::{HashingWriter, Sha256, Sha256Hash};

use crate::{path_for_hash, temp_dir};

/// A synchronous writer for writing content to the CAS.
///
/// Content is written to a temporary file, and when `finish()` is called,
/// the file is moved to its final location based on its content hash.
pub struct SyncWriter {
    root: PathBuf,
    temp_path: tempfile::TempPath,
    writer: HashingWriter<std::fs::File, Sha256>,
}

impl SyncWriter {
    /// Opens a new temporary file in the CAS and returns a writer that can
    /// be used to write to it.
    ///
    /// Make sure to call `finish` to actually write the file to the CAS.
    /// Without it, the file will be deleted.
    ///
    /// # Arguments
    ///
    /// * `root` - The root directory of the CAS store
    pub fn create(root: &Path) -> std::io::Result<Self> {
        // Ensure the temporary directory exists.
        let temp_dir = temp_dir(root);
        fs::create_dir_all(&temp_dir)?;

        // Open a temporary file at the root of the cas.
        let builder = tempfile::Builder::new();
        let temp_file = builder.tempfile_in(&temp_dir)?;
        let (file, temp_path) = temp_file.into_parts();
        Ok(SyncWriter {
            root: root.to_path_buf(),
            temp_path,
            writer: HashingWriter::new(file),
        })
    }

    /// Store the file in the cache at the right location based on the hash of
    /// the bytes that were written.
    pub fn finish(mut self) -> std::io::Result<Sha256Hash> {
        self.writer.flush()?;

        // Finalize any writing and compute the final hash of the file.
        let (file, hash) = self.writer.finalize();

        // Compute the final location of the file
        let path = self.root.join(path_for_hash(&hash));

        // Ensure the parent directory exists.
        fs::create_dir_all(path.parent().expect("parent directory must exist"))?;

        // Drop the remaining file handle to ensure there is no more open handle.
        drop(file);

        // Persist the file to disk but ignore if the file already exists.
        match self
            .temp_path
            .persist_noclobber(&path)
            .map_err(std::io::Error::from)
        {
            Ok(_) => Ok(hash),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(hash),
            Err(e) => Err(e),
        }
    }
}

impl Write for SyncWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.writer.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}
