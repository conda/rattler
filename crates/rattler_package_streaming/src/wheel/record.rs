//! Original-wheel integrity checks and RECORD generation for the cache layout.

use std::{
    collections::HashMap,
    io::{self, Read, Seek},
    path::Path,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rattler_conda_types::package::PathsEntry;
use sha2::{Sha256, Sha384, Sha512, digest::DynDigest};
use zip::ZipArchive;

use super::SITE_PACKAGES;
use crate::ExtractError;

/// Validates supplied hashes and sizes; permits unlisted files and empty hashes.
pub(super) struct RecordManifest {
    entries: HashMap<String, RecordEntry>,
}

struct RecordEntry {
    hash: Option<RecordHash>,
    size: Option<u64>,
}

struct RecordHash {
    algorithm: String,
    digest: Vec<u8>,
}

impl RecordHash {
    fn hasher(&self) -> Box<dyn DynDigest> {
        match self.algorithm.as_str() {
            "sha256" => Box::new(Sha256::default()),
            "sha384" => Box::new(Sha384::default()),
            "sha512" => Box::new(Sha512::default()),
            _ => unreachable!("RECORD algorithms are validated when parsing"),
        }
    }
}

impl RecordManifest {
    pub(super) fn read<R: Read + Seek>(
        archive: &mut ZipArchive<R>,
        dist_info: &Path,
    ) -> Result<Self, ExtractError> {
        let record_name = format!("{}/RECORD", posix_path(dist_info));
        let index = archive
            .index_for_name(&record_name)
            .ok_or_else(|| ExtractError::InvalidWheel(format!("{record_name} is missing")))?;
        let mut csv = csv::ReaderBuilder::new()
            .has_headers(false)
            .from_reader(archive.by_index(index)?);
        let mut entries = HashMap::new();
        for row in csv.records() {
            let row =
                row.map_err(|err| ExtractError::InvalidWheel(format!("malformed RECORD: {err}")))?;
            if row.len() != 3 {
                return Err(ExtractError::InvalidWheel(
                    "RECORD rows must contain exactly three fields".into(),
                ));
            }
            let path = row[0].to_string();
            let hash = &row[1];
            let size = &row[2];
            if path.is_empty() {
                return Err(ExtractError::InvalidWheel("empty RECORD path".into()));
            }
            let hash = if hash.is_empty() {
                None
            } else {
                let (algorithm, encoded) = hash.split_once('=').ok_or_else(|| {
                    ExtractError::InvalidWheel(format!("invalid RECORD hash for {path}"))
                })?;
                let expected_len = match algorithm {
                    "sha256" => 32,
                    "sha384" => 48,
                    "sha512" => 64,
                    _ => {
                        return Err(ExtractError::InvalidWheel(format!(
                            "unsupported RECORD hash algorithm '{algorithm}' for {path}"
                        )));
                    }
                };
                let digest = URL_SAFE_NO_PAD.decode(encoded).map_err(|err| {
                    ExtractError::InvalidWheel(format!("invalid RECORD hash for {path}: {err}"))
                })?;
                if digest.len() != expected_len {
                    return Err(ExtractError::InvalidWheel(format!(
                        "RECORD has a {algorithm} of {} bytes for {path}",
                        digest.len()
                    )));
                }
                Some(RecordHash {
                    algorithm: algorithm.into(),
                    digest,
                })
            };
            let size = if size.is_empty() {
                None
            } else {
                Some(size.parse::<u64>().map_err(|err| {
                    ExtractError::InvalidWheel(format!("invalid RECORD size for {path}: {err}"))
                })?)
            };
            if entries
                .insert(path.clone(), RecordEntry { hash, size })
                .is_some()
            {
                return Err(ExtractError::InvalidWheel(format!(
                    "duplicate RECORD path: {path}"
                )));
            }
        }
        Ok(Self { entries })
    }

    pub(super) fn reader<'a, R: Read>(
        &'a self,
        path: &'a Path,
        reader: R,
    ) -> VerifyingReader<'a, R> {
        let claimed = self.entries.get(&posix_path(path));
        VerifyingReader {
            reader,
            path,
            claimed,
            hasher: claimed
                .and_then(|entry| entry.hash.as_ref())
                .map(RecordHash::hasher),
            size: 0,
        }
    }
}

/// Checks original bytes, including replaced metadata. Call `verify` after EOF.
pub(super) struct VerifyingReader<'a, R> {
    reader: R,
    path: &'a Path,
    claimed: Option<&'a RecordEntry>,
    hasher: Option<Box<dyn DynDigest>>,
    size: u64,
}

impl<R: Read> Read for VerifyingReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = self.reader.read(buf)?;
        if let Some(hasher) = &mut self.hasher {
            hasher.update(&buf[..len]);
        }
        self.size += len as u64;
        Ok(len)
    }
}

impl<R> VerifyingReader<'_, R> {
    pub(super) fn verify(self) -> Result<(), ExtractError> {
        let Some(claimed) = self.claimed else {
            return Ok(());
        };
        if let (Some(expected), Some(hasher)) = (&claimed.hash, self.hasher)
            && hasher.finalize().as_ref() != expected.digest.as_slice()
        {
            return Err(ExtractError::InvalidWheel(format!(
                "{}: RECORD {} mismatch",
                self.path.display(),
                expected.algorithm
            )));
        }
        if let Some(expected) = claimed.size
            && expected != self.size
        {
            return Err(ExtractError::InvalidWheel(format!(
                "{}: RECORD claims {expected} bytes but the archive holds {}",
                self.path.display(),
                self.size
            )));
        }
        Ok(())
    }
}

/// Builds RECORD relative to the cache's `site-packages`, not an installed prefix.
pub(super) fn for_cache(paths: &[PathsEntry], dist_info: &Path) -> Result<Vec<u8>, ExtractError> {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());
    for entry in paths {
        let relative = match entry.relative_path.strip_prefix(SITE_PACKAGES) {
            Ok(path) => posix_path(path),
            Err(_) => format!("../{}", posix_path(&entry.relative_path)),
        };
        let hash = entry
            .sha256
            .map(|hash| format!("sha256={}", URL_SAFE_NO_PAD.encode(hash)))
            .unwrap_or_default();
        writer
            .serialize((relative, hash, entry.size_in_bytes))
            .map_err(io::Error::other)?;
    }
    writer
        .write_record([
            format!("{}/RECORD", posix_path(dist_info)),
            String::new(),
            String::new(),
        ])
        .map_err(io::Error::other)?;
    writer
        .into_inner()
        .map_err(|err| io::Error::other(err).into())
}

fn posix_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
