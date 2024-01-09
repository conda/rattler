#![deny(missing_docs)]

//! A module that provides utility functions for computing hashes using the
//! [RustCrypto/hashes](https://github.com/RustCrypto/hashes) library.
//!
//! This module provides several functions that wrap around the hashing algorithms provided by the
//! `RustCrypto` library. These functions allow you to easily compute the hash of a file, or a
//! stream of bytes using a variety of hashing algorithms.
//!
//! By utilizing the [`Digest`] trait, any hashing algorithm that implements that trait can be used
//! with the functions provided in this crate.
//!
//! # Examples
//!
//! ```no_run
//! use rattler_digest::{compute_bytes_digest, compute_file_digest};
//! use sha2::Sha256;
//! use md5::Md5;
//!
//! // Compute the MD5 hash of a string
//! let md5_result = compute_bytes_digest::<Md5>("Hello, world!");
//! println!("MD5 hash: {:x}", md5_result);
//!
//! // Compute the SHA256 hash of a file
//! let sha256_result = compute_file_digest::<Sha256>("somefile.txt").unwrap();
//! println!("SHA256 hash: {:x}", sha256_result);
//! ```
//!
//! # Available functions
//!
//! - [`compute_file_digest`]: Computes the hash of a file on disk.
//! - [`parse_digest_from_hex`]: Given a hex representation of a digest, parses it to bytes.
//! - [`HashingWriter`]: An object that wraps a writable object and implements [`Write`] and
//!   [`::tokio::io::AsyncWrite`]. It forwards the data to the wrapped object but also computes the hash of the
//!   content on the fly.
//!
//! For more information on the hashing algorithms provided by the
//! [RustCrypto/hashes](https://github.com/RustCrypto/hashes) library, see the documentation for
//! that library.

#[cfg(feature = "tokio")]
mod tokio;

#[cfg(feature = "serde")]
pub mod serde;

pub use digest;

use blake2::digest::consts::U32;
use blake2::{Blake2b, Blake2bMac};
use digest::{Digest, Output};
use std::io::Read;
use std::{fs::File, io::Write, path::Path};

pub use md5::Md5;
pub use sha2::Sha256;

/// A type alias for the output of a SHA256 hash.
pub type Sha256Hash = sha2::digest::Output<Sha256>;

/// A type alias for the output of an MD5 hash.
pub type Md5Hash = md5::digest::Output<Md5>;

/// A type for a 32 bit length blake2b digest.
pub type Blake2b256 = Blake2b<U32>;

/// A type alias for the output of a [`Blake2b256`] hash.
pub type Blake2b256Hash = blake2::digest::Output<Blake2b256>;

/// A type alias for the output of a blake2b256 hash.
pub type Blake2bMac256 = Blake2bMac<U32>;

/// A type alias for the output of a [`Blake2bMac256`] hash.
pub type Blake2bMac256Hash = blake2::digest::Output<Blake2bMac256>;

/// Compute a hash of the file at the specified location.
pub fn compute_file_digest<D: Digest + Default + Write>(
    path: impl AsRef<Path>,
) -> Result<Output<D>, std::io::Error> {
    // Open the file for reading
    let mut file = File::open(path)?;

    // Determine the hash of the file on disk
    let mut hasher = D::default();
    std::io::copy(&mut file, &mut hasher)?;

    Ok(hasher.finalize())
}

/// Compute a hash of the specified bytes.
pub fn compute_bytes_digest<D: Digest + Default + Write>(bytes: impl AsRef<[u8]>) -> Output<D> {
    let mut hasher = D::default();
    hasher.update(bytes);
    hasher.finalize()
}

/// Parses a hash hex string to a digest.
pub fn parse_digest_from_hex<D: Digest>(str: &str) -> Option<Output<D>> {
    let mut hash = <Output<D>>::default();
    match hex::decode_to_slice(str, &mut hash) {
        Ok(_) => Some(hash),
        Err(_) => None,
    }
}

/// A simple object that provides a [`Write`] implementation that also immediately hashes the bytes
/// written to it. Call [`HashingWriter::finalize`] to retrieve both the original `impl Write`
/// object as well as the hash.
///
/// If the `tokio` feature is enabled this object also implements [`::tokio::io::AsyncWrite`] which
/// allows you to use it in an async context as well.
pub struct HashingWriter<W, D: Digest> {
    writer: W,
    hasher: D,
}

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
    /// Consumes this instance and returns the original writer and the hash of all bytes written to
    /// this instance.
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

/// A simple object that provides a [`Read`] implementation that also immediately hashes the bytes
/// read from it. Call [`HashingReader::finalize`] to retrieve both the original `impl Read`
/// object as well as the hash.
///
/// If the `tokio` feature is enabled this object also implements [`::tokio::io::AsyncRead`] which
/// allows you to use it in an async context as well.
pub struct HashingReader<R, D: Digest> {
    reader: R,
    hasher: D,
}

impl<R, D: Digest + Default> HashingReader<R, D> {
    /// Constructs a new instance from a reader and a new (empty) hasher.
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            hasher: Default::default(),
        }
    }
}

impl<R, D: Digest> HashingReader<R, D> {
    /// Consumes this instance and returns the original reader and the hash of all bytes read from
    /// this instance.
    pub fn finalize(self) -> (R, Output<D>) {
        (self.reader, self.hasher.finalize())
    }
}

impl<R: Read, D: Digest> Read for HashingReader<R, D> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let bytes_read = self.reader.read(buf)?;
        self.hasher.update(&buf[..bytes_read]);
        Ok(bytes_read)
    }
}

#[cfg(test)]
mod test {
    use super::HashingReader;
    use rstest::rstest;
    use sha2::Sha256;
    use std::io::Read;

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
        let hash = super::compute_file_digest::<sha2::Sha256>(&file_path).unwrap();

        assert_eq!(format!("{hash:x}"), expected_hash);
    }

    #[rstest]
    #[case(
        "1234567890",
        "c775e7b757ede630cd0aa1113bd102661ab38829ca52a6422ab782862f268646"
    )]
    #[case(
        "Hello, world!",
        "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3"
    )]
    fn test_hashing_reader_sha256(#[case] input: &str, #[case] expected_hash: &str) {
        let mut cursor = HashingReader::<_, Sha256>::new(std::io::Cursor::new(input));
        let mut cursor_string = String::new();
        cursor.read_to_string(&mut cursor_string).unwrap();
        assert_eq!(&cursor_string, input);
        let (_, hash) = cursor.finalize();
        assert_eq!(format!("{hash:x}"), expected_hash);
    }
}
