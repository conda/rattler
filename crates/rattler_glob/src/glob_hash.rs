//! This module contains the `GlobHash` struct which is used to calculate a hash of the files that match the given glob patterns.
//! Use this if you want to calculate a hash of a set of files that match a glob pattern.
//! This is useful for finding out if you need to rebuild a target based on the files that match a glob pattern.
use std::{
    fs::File,
    io::{self, BufRead, Read, Write},
    path::{Path, PathBuf},
};

use rattler_digest::{digest::Digest, Sha256, Sha256Hash};
use thiserror::Error;

use crate::{GlobSet, GlobSetError};

/// Contains a hash of the files that match the given glob patterns.
#[derive(Debug, Clone, Default)]
pub struct GlobHash {
    /// The hash of the files that match the given glob patterns.
    pub hash: Sha256Hash,
    #[cfg(test)]
    matching_files: Vec<String>,
}

/// Errors that can occur when computing a glob hash.
#[derive(Error, Debug)]
pub enum GlobHashError {
    /// Failed to normalize line endings while reading a file.
    #[error("during line normalization, failed to access {}", .0.display())]
    NormalizeLineEnds(PathBuf, #[source] io::Error),

    /// The hash computation was cancelled (e.g., task was aborted).
    #[error("the operation was cancelled")]
    Cancelled,

    /// An error occurred while building or walking the glob set.
    #[error(transparent)]
    GlobSetIgnore(#[from] GlobSetError),
}

impl GlobHash {
    /// Calculate a hash of the files that match the given glob patterns.
    ///
    /// This function walks the directory tree starting from `root_dir`, finds all files
    /// matching the provided glob patterns, and computes a combined SHA-256 hash of their
    /// paths and contents. The hash is computed deterministically (files are sorted by path).
    ///
    /// Line endings are normalized during hashing: `\r\n` sequences are converted to `\n`
    /// in text files, while binary files (detected by the presence of null bytes) are
    /// hashed verbatim.
    ///
    /// # Arguments
    /// * `root_dir` - The root directory to search from
    /// * `globs` - An iterator of glob patterns (supports gitignore-style syntax)
    ///
    /// # Returns
    /// A `GlobHash` containing the computed hash, or an error if the operation failed.
    ///
    /// # Example
    /// ```no_run
    /// use rattler_glob::GlobHash;
    /// use std::path::Path;
    ///
    /// let hash = GlobHash::from_patterns(
    ///     Path::new("/my/project"),
    ///     ["src/**/*.rs", "!src/generated/**"],
    /// ).unwrap();
    ///
    /// println!("Hash: {:x}", hash.hash);
    /// ```
    pub fn from_patterns<'a>(
        root_dir: &Path,
        globs: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, GlobHashError> {
        // If the root is not a directory or does not exist, return an empty map.
        if !root_dir.is_dir() {
            return Ok(Self::default());
        }

        let glob_set = GlobSet::create(globs)?;
        // Collect matching entries and convert to concrete DirEntry list, propagating errors.
        let mut entries = glob_set.collect_matching(root_dir)?;

        // Sort deterministically by path
        entries.sort_by_key(|e| e.path().to_path_buf());

        #[cfg(test)]
        let mut matching_files = Vec::new();

        let mut hasher = Sha256::default();
        for entry in entries {
            // Construct a normalized file path to ensure consistent hashing across
            // platforms. And add it to the hash.
            let relative_path = entry.path().strip_prefix(root_dir).unwrap_or(entry.path());
            let normalized_file_path = relative_path.to_string_lossy().replace("\\", "/");
            rattler_digest::digest::Update::update(&mut hasher, normalized_file_path.as_bytes());

            #[cfg(test)]
            matching_files.push(normalized_file_path);

            // Concatenate the contents of the file to the hash.
            File::open(entry.path())
                .and_then(|mut file| normalize_line_endings(&mut file, &mut hasher))
                .map_err(move |e| {
                    GlobHashError::NormalizeLineEnds(entry.path().to_path_buf(), e)
                })?;
        }

        let hash = hasher.finalize();

        Ok(Self {
            hash,
            #[cfg(test)]
            matching_files,
        })
    }
}

/// This function copies the contents of the reader to the writer but normalizes
/// the line endings (e.g. replaces `\r\n` with `\n`) in text files.
fn normalize_line_endings<R: Read, W: Write>(reader: &mut R, writer: &mut W) -> io::Result<()> {
    let mut reader = io::BufReader::new(reader);

    // Check if binary by looking for null bytes
    let buffer = reader.fill_buf()?;
    if buffer.contains(&0) {
        std::io::copy(&mut reader, writer)?;
        return Ok(());
    }

    let mut pending_cr = false;

    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            break;
        }

        let mut written_to = 0;

        for (i, &byte) in buffer.iter().enumerate() {
            if byte == b'\r' {
                // Flush any previous pending \r (it was standalone)
                if pending_cr {
                    writer.write_all(b"\r")?;
                }
                // Write everything up to this \r
                writer.write_all(&buffer[written_to..i])?;
                written_to = i + 1;
                pending_cr = true;
            } else if byte == b'\n' {
                // Write everything up to this \n
                writer.write_all(&buffer[written_to..i])?;
                written_to = i + 1;
                // Write \n - pending \r is discarded (normalizes \r\n → \n)
                writer.write_all(b"\n")?;
                pending_cr = false;
            } else if pending_cr {
                // Previous \r was standalone, write it now
                writer.write_all(b"\r")?;
                pending_cr = false;
            }
        }

        // Write remaining data in buffer
        writer.write_all(&buffer[written_to..])?;

        let len = buffer.len();
        reader.consume(len);
    }

    // Handle trailing \r at EOF
    if pending_cr {
        writer.write_all(b"\r")?;
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use std::path::Path;

    use itertools::Itertools;
    use rstest::*;

    use super::*;

    #[fixture]
    pub fn testname() -> String {
        let thread_name = std::thread::current().name().unwrap().to_string();
        let test_name = thread_name.rsplit("::").next().unwrap_or(&thread_name);
        format!("glob_hash_{test_name}")
    }

    #[rstest]
    #[case::satisfiability(vec!["tests/data/satisfiability/source-dependency/**/*"])]
    #[case::satisfiability_ignore_lock(vec!["tests/data/satisfiability/source-dependency/**/*", "!tests/data/satisfiability/source-dependency/**/*.lock"])]
    #[case::non_glob(vec!["tests/data/satisfiability/source-dependency/pixi.toml"])]
    fn test_input_hash(testname: String, #[case] globs: Vec<&str>) {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap();
        let glob_hash = GlobHash::from_patterns(root_dir, globs.iter().copied()).unwrap();
        let snapshot = format!(
            "Globs:\n{}\nHash: {:x}\nMatched files:\n{}",
            globs
                .iter()
                .format_with("\n", |glob, f| f(&format_args!("- {glob}"))),
            glob_hash.hash,
            glob_hash
                .matching_files
                .iter()
                .format_with("\n", |glob, f| f(&format_args!("- {glob}")))
        );
        insta::assert_snapshot!(testname, snapshot);
    }

    #[test]
    fn test_normalize_line_endings() {
        let input =
            "\rHello\r\nWorld\r\nYou are the best\nThere is no-one\r\r \rlike you.\r".repeat(8196);
        let mut normalized: Vec<u8> = Vec::new();
        normalize_line_endings(&mut input.as_bytes(), &mut normalized).unwrap();
        let output = String::from_utf8(normalized).unwrap();
        assert_eq!(output, input.replace("\r\n", "\n"));
    }

    /// A reader that returns data in small chunks, used to test buffer boundary behavior.
    struct ChunkedReader<'a> {
        data: &'a [u8],
        chunk_size: usize,
    }

    impl<'a> Read for ChunkedReader<'a> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let len = std::cmp::min(self.chunk_size, std::cmp::min(buf.len(), self.data.len()));
            if len == 0 {
                return Ok(0);
            }
            buf[..len].copy_from_slice(&self.data[..len]);
            self.data = &self.data[len..];
            Ok(len)
        }
    }

    #[test]
    fn test_crlf_spanning_buffer_boundary() {
        // Test case where \r\n spans across buffer boundaries.
        // We use a chunk size of 1 to ensure \r and \n are in different reads.
        let input = b"Hello\r\nWorld\r\nEnd";
        let mut reader = ChunkedReader {
            data: input,
            chunk_size: 1, // Force each byte to be read separately
        };
        let mut output: Vec<u8> = Vec::new();
        normalize_line_endings(&mut reader, &mut output).unwrap();
        assert_eq!(output, b"Hello\nWorld\nEnd");
    }

    #[test]
    fn test_standalone_cr_across_boundary() {
        // Test that standalone \r (not followed by \n) is preserved even across boundaries.
        let input = b"Hello\rWorld";
        let mut reader = ChunkedReader {
            data: input,
            chunk_size: 1,
        };
        let mut output: Vec<u8> = Vec::new();
        normalize_line_endings(&mut reader, &mut output).unwrap();
        assert_eq!(output, b"Hello\rWorld");
    }

    #[test]
    fn test_cr_at_end_of_input() {
        // Test that \r at the very end of input is preserved.
        let input = b"Hello\r";
        let mut reader = ChunkedReader {
            data: input,
            chunk_size: 1,
        };
        let mut output: Vec<u8> = Vec::new();
        normalize_line_endings(&mut reader, &mut output).unwrap();
        assert_eq!(output, b"Hello\r");
    }
}
