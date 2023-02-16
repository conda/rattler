use blake2::Blake2s256;
use digest::{Digest, Output};
use sha2::Sha256;
use std::fs::File;
use std::io::{Error, Write};
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;

/// Compute the SHA256 hash of the file at the specified location.
pub fn compute_file_sha256(path: &Path) -> Result<Output<Sha256>, std::io::Error> {
    compute_file_hash::<Sha256>(path)
}

/// Compute the Blake2 hash of the file at the specified location.
pub fn compute_file_blake2(path: &Path) -> Result<Output<Blake2s256>, std::io::Error> {
    compute_file_hash::<Blake2s256>(path)
}

/// Compute a hash of the file at the specified location.
pub fn compute_file_hash<D: Digest + Default + Write>(
    path: &Path,
) -> Result<Output<D>, std::io::Error> {
    // Open the file for reading
    let mut file = File::open(path)?;

    // Determine the hash of the file on disk
    let mut hasher = D::default();
    std::io::copy(&mut file, &mut hasher)?;

    Ok(hasher.finalize())
}

/// Parses a SHA256 hex string to a digest.
pub fn parse_sha256_from_hex(str: &str) -> Option<Output<Sha256>> {
    let mut sha256 = <Output<Sha256>>::default();
    match hex::decode_to_slice(str, &mut sha256) {
        Ok(_) => Some(sha256),
        Err(_) => None,
    }
}

/// A simple object that provides a [`Write`] implementation that also immediately hashes the bytes
/// written to it.
pub struct HashingWriter<W, D: Digest> {
    writer: W,
    hasher: D,
}

pub type Sha256HashingWriter<T> = HashingWriter<T, sha2::Sha256>;
pub type Blake2s256HashingWriter<T> = HashingWriter<T, blake2::Blake2s256>;

impl<W, D: Digest + Default> HashingWriter<W, D> {
    /// Constructs a new instance from a writer and a new (empty) hasher.
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            hasher: Default::default(),
        }
    }
}

impl<W, D: Digest> HashingWriter<W, D> {
    pub fn finalize(self) -> (W, Output<D>) {
        (self.writer, self.hasher.finalize())
    }
}

impl<W: Write, D: Digest> Write for HashingWriter<W, D> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let bytes = self.writer.write(buf)?;
        self.hasher.update(&buf[..bytes]);
        Ok(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

impl<W: AsyncWrite + Unpin, D: Digest> AsyncWrite for HashingWriter<W, D> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        // pin-project the writer
        let (writer, hasher) = unsafe {
            let this = self.get_unchecked_mut();
            (Pin::new_unchecked(&mut this.writer), &mut this.hasher)
        };

        match writer.poll_write(cx, buf) {
            Poll::Ready(Ok(bytes)) => {
                hasher.update(&buf[..bytes]);
                Poll::Ready(Ok(bytes))
            }
            other => other,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        // This is okay because `writer` is pinned when `self` is.
        let writer = unsafe { self.map_unchecked_mut(|s| &mut s.writer) };
        writer.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        // This is okay because `writer` is pinned when `self` is.
        let writer = unsafe { self.map_unchecked_mut(|s| &mut s.writer) };
        writer.poll_flush(cx)
    }
}

#[cfg(test)]
mod test {
    use rstest::rstest;

    #[rstest]
    #[case(
        "1234567890",
        "c775e7b757ede630cd0aa1113bd102661ab38829ca52a6422ab782862f268646"
    )]
    #[case(
        "Hello, world!",
        "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3"
    )]
    fn test_compute_file_sha256(#[case] input: &str, #[case] expected_hash: &str) {
        // Write a known value to a temporary file and verify that the compute hash matches what we would
        // expect.

        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test");
        std::fs::write(&file_path, input).unwrap();
        let hash = super::compute_file_sha256(&file_path).unwrap();

        assert_eq!(format!("{hash:x}"), expected_hash)
    }
}
