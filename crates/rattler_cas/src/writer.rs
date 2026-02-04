//! Async writer for the Content Addressable Store.

use std::{
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
};

use fs_err::tokio as fs;
use pin_project_lite::pin_project;
use rattler_digest::{HashingWriter, Sha256, Sha256Hash};
use tokio::io::AsyncWriteExt;

use crate::{path_for_hash, temp_dir};

pin_project! {
    /// An async writer for writing content to the CAS.
    ///
    /// Content is written to a temporary file, and when `finish()` is called,
    /// the file is moved to its final location based on its content hash.
    pub struct Writer {
        root: PathBuf,
        temp_path: tempfile::TempPath,
        #[pin]
        writer: HashingWriter<tokio::fs::File, Sha256>,
    }
}

impl Writer {
    /// Opens a new temporary file in the CAS and returns an async writer that
    /// can be used to write to it asynchronously.
    ///
    /// Make sure to call `finish` to actually write the file to the CAS.
    /// Without it, the file will be deleted.
    ///
    /// # Arguments
    ///
    /// * `root` - The root directory of the CAS store
    pub async fn create(root: &Path) -> std::io::Result<Self> {
        // Ensure the temporary directory exists.
        let temp_dir = temp_dir(root);
        fs::create_dir_all(&temp_dir).await?;

        // Open a temporary file at the root of the cas.
        let builder = tempfile::Builder::new();
        let temp_file = builder.tempfile_in(&temp_dir)?;
        let (file, temp_path) = temp_file.into_parts();

        // Convert the std::fs::File to tokio::fs::File
        let async_file = tokio::fs::File::from_std(file);

        Ok(Writer {
            root: root.to_path_buf(),
            temp_path,
            writer: HashingWriter::new(async_file),
        })
    }

    /// Store the file in the cache at the right location based on the hash of
    /// the bytes that were written.
    pub async fn finish(mut self) -> std::io::Result<Sha256Hash> {
        // Flush the contents of the file.
        self.writer.flush().await?;

        // Finalize any writing and compute the final hash of the file.
        let (file, hash) = self.writer.finalize();

        // Compute the final location of the file
        let path = self.root.join(path_for_hash(&hash));
        let temp_path = self.temp_path;

        match tokio::task::spawn_blocking(move || {
            // Ensure the parent directory exists.
            fs_err::create_dir_all(path.parent().expect("parent directory must exist"))?;

            // Drop the remaining file handle to ensure there is no more open handle.
            drop(file);

            // Persist the file to disk but ignore if the file already exists.
            temp_path
                .persist_noclobber(&path)
                .map_err(std::io::Error::from)
        })
        .await
        .map_err(tokio::task::JoinError::try_into_panic)
        {
            Ok(Ok(_)) => Ok(hash),
            Ok(Err(err)) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(hash),
            Ok(Err(e)) => Err(e),
            Err(Err(e)) => Err(std::io::Error::new(std::io::ErrorKind::Interrupted, e)),
            Err(Ok(panic)) => std::panic::resume_unwind(panic),
        }
    }
}

impl tokio::io::AsyncWrite for Writer {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.project().writer.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.project().writer.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.project().writer.poll_shutdown(cx)
    }
}
