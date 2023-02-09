use digest::{Digest, Output};
use sha2::Sha256;
use std::fs::File;
use std::io::Write;
use std::path::Path;

/// Compute the SHA256 hash of the file at the specified location.
pub fn compute_file_sha256(path: &Path) -> Result<sha2::digest::Output<Sha256>, std::io::Error> {
    // Open the file for reading
    let mut file = File::open(path)?;

    // Determine the hash of the file on disk
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;

    Ok(hasher.finalize())
}

/// A simple object that provides a [`Write`] implementation that also immediately hashes the bytes
/// written to it.
pub struct HashingWriter<W: Write, D: Digest> {
    writer: W,
    hasher: D,
}

pub type Sha256HashingWriter<T> = HashingWriter<T, sha2::Sha256>;

impl<W: Write, D: Digest + Default> HashingWriter<W, D> {
    /// Constructs a new instance from a writer and a new (empty) hasher.
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            hasher: Default::default(),
        }
    }
}

impl<W: Write, D: Digest> HashingWriter<W, D> {
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
