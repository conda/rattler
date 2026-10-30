//! Destination-aware RECORD generation for converted wheels.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{self, Write},
    path::{Component, Path},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rattler_conda_types::{
    PackageName, PrefixRecord, RepoDataRecord,
    package::DistArchiveType,
    prefix::Prefix,
    prefix_record::{PathType, PathsEntry},
};
use rattler_digest::{Sha256, Sha256Hash, compute_bytes_digest, compute_file_digest};

/// Finalize the transaction's wheel records using metadata reloaded after
/// clobber resolution and link scripts.
pub(super) fn finalize_wheel_records(
    prefix: &Prefix,
    records: &mut [PrefixRecord],
    ignored: Option<&HashSet<PackageName>>,
    rehash: bool,
) -> io::Result<()> {
    if !records
        .iter()
        .any(|record| is_managed_wheel(&record.repodata_record, ignored))
    {
        return Ok(());
    }
    // Clobber resolution can update ordinary conda owners as well as wheels.
    for record in records.iter_mut() {
        *record = PrefixRecord::from_path(prefix.join("conda-meta").join(record.file_name()))?;
    }
    validate_wheel_ownership(records, ignored)?;
    for record in records
        .iter_mut()
        .filter(|record| is_managed_wheel(&record.repodata_record, ignored))
    {
        finalize_wheel_record(prefix, record, rehash)?;
    }
    Ok(())
}

/// Rewrite one wheel's `RECORD` from its owned paths and reconcile the hashes
/// and sizes stored in `conda-meta` with the installed bytes.
fn finalize_wheel_record(
    prefix: &Prefix,
    record: &mut PrefixRecord,
    rehash: bool,
) -> io::Result<()> {
    let record_index = find_record_entry(record)?;
    let record_path = record.paths_data.paths[record_index].relative_path.clone();
    validate_relative(&record_path)?;
    let dist_info = record_path
        .parent()
        .ok_or_else(|| invalid("invalid wheel RECORD location"))?;
    // RECORD rows are relative to the directory that contains `.dist-info`.
    let base = dist_info
        .parent()
        .ok_or_else(|| invalid("invalid wheel RECORD location"))?;

    let mut rows = BTreeMap::new();
    let mut metadata_changed = false;
    for (index, entry) in record.paths_data.paths.iter_mut().enumerate() {
        validate_relative(&entry.relative_path)?;
        if entry.path_type == PathType::Directory {
            continue;
        }
        let relative = pathdiff::diff_paths(&entry.relative_path, base)
            .ok_or_else(|| invalid("cannot compute relative RECORD path"))?;
        let relative = posix_path(&relative)?;
        let row = if index == record_index {
            // The RECORD file never lists its own hash and size.
            (String::new(), String::new())
        } else {
            let (hash, size) = installed_hash_and_size(prefix, entry, rehash)?;
            metadata_changed |= reconcile_installed_hash(entry, hash, size);
            (
                format!("sha256={}", URL_SAFE_NO_PAD.encode(hash)),
                size.to_string(),
            )
        };
        if rows.insert(relative, row).is_some() {
            return Err(invalid("duplicate owned path in wheel RECORD"));
        }
    }

    let bytes = encode_record(rows)?;
    write_record_file(prefix, dist_info, &prefix.join(&record_path), &bytes)?;

    let entry = &mut record.paths_data.paths[record_index];
    // Repair stale conda-meta hashes even when RECORD is already up to date.
    metadata_changed |= reconcile_installed_hash(
        entry,
        compute_bytes_digest::<Sha256>(&bytes),
        bytes.len() as u64,
    );
    if metadata_changed {
        record.write_to_path(prefix.join("conda-meta").join(record.file_name()), true)?;
    }
    Ok(())
}

/// Find the distribution's unique `RECORD` by name and version, independent
/// of the current Python layout.
fn find_record_entry(record: &PrefixRecord) -> io::Result<usize> {
    let expected_name = normalize_name(record.repodata_record.package_record.name.as_source());
    let expected_version: pep440_rs::Version = record
        .repodata_record
        .package_record
        .version
        .to_string()
        .parse()
        .map_err(|error| invalid(format!("invalid wheel version: {error}")))?;
    let candidates: Vec<_> = record
        .paths_data
        .paths
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            // Use the original path to identify a clobbered RECORD.
            let logical = entry.original_path.as_ref().unwrap_or(&entry.relative_path);
            if logical.file_name()? != "RECORD" {
                return None;
            }
            let stem = logical
                .parent()?
                .file_name()?
                .to_str()?
                .strip_suffix(".dist-info")?;
            let (name, version) = stem.rsplit_once('-')?;
            (normalize_name(name) == expected_name
                && version.parse::<pep440_rs::Version>().ok()? == expected_version)
                .then_some(index)
        })
        .collect();
    if let [record_index] = candidates.as_slice() {
        Ok(*record_index)
    } else {
        Err(invalid(format!(
            "expected one unambiguous wheel RECORD for {}",
            record.file_name()
        )))
    }
}

/// Return the installed hash and size, reusing the recorded hash when the
/// size matches and `rehash` is false.
fn installed_hash_and_size(
    prefix: &Prefix,
    entry: &PathsEntry,
    rehash: bool,
) -> io::Result<(Sha256Hash, u64)> {
    let path = prefix.join(&entry.relative_path);
    let metadata = fs_err::metadata(&path)?;
    if !metadata.is_file() {
        return Err(invalid(format!(
            "wheel payload is not a file: {}",
            path.display()
        )));
    }
    let size = metadata.len();
    let hash = match entry.sha256_in_prefix.or(entry.sha256) {
        Some(hash) if !rehash && entry.size_in_bytes == Some(size) => hash,
        _ => compute_file_digest::<Sha256>(&path)?,
    };
    Ok((hash, size))
}

/// Update installed metadata and return whether it changed.
/// Keep `sha256_in_prefix` only when it differs from the archive hash.
fn reconcile_installed_hash(entry: &mut PathsEntry, hash: Sha256Hash, size: u64) -> bool {
    let installed_hash = (entry.sha256 != Some(hash)).then_some(hash);
    if entry.sha256_in_prefix == installed_hash && entry.size_in_bytes == Some(size) {
        return false;
    }
    entry.sha256_in_prefix = installed_hash;
    entry.size_in_bytes = Some(size);
    true
}

fn encode_record(rows: BTreeMap<String, (String, String)>) -> io::Result<Vec<u8>> {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());
    for (path, (hash, size)) in rows {
        writer
            .write_record([path, hash, size])
            .map_err(io::Error::other)?;
    }
    writer.into_inner().map_err(io::Error::other)
}

/// Atomically replace `destination` with `bytes`, keeping its permissions.
/// `dist_info` is the destination's directory relative to the prefix.
fn write_record_file(
    prefix: &Prefix,
    dist_info: &Path,
    destination: &Path,
    bytes: &[u8],
) -> io::Result<()> {
    // Directory symlinks could redirect writes into the cache. File symlinks
    // are safe because atomic replacement replaces the link itself.
    let mut directory = prefix.path().to_path_buf();
    for component in dist_info.components() {
        directory.push(component);
        if fs_err::symlink_metadata(&directory)?
            .file_type()
            .is_symlink()
        {
            return Err(invalid(format!(
                "RECORD parent is a symlink: {}",
                directory.display()
            )));
        }
    }
    if fs_err::read(destination)? == bytes {
        return Ok(());
    }
    let permissions = fs_err::metadata(destination)?.permissions();
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(bytes)?;
    temporary.as_file().set_permissions(permissions)?;
    temporary
        .persist(destination)
        .map_err(|error| error.error)?;
    Ok(())
}

/// Whether `record` is a converted wheel whose `RECORD` this module owns.
pub(super) fn is_managed_wheel(
    record: &RepoDataRecord,
    ignored: Option<&HashSet<PackageName>>,
) -> bool {
    let is_wheel = match record.identifier.archive_type {
        DistArchiveType::Wheel(_) => true,
        DistArchiveType::Conda(_) => false,
    };
    is_wheel && ignored.is_none_or(|ignored| !ignored.contains(&record.package_record.name))
}

/// Reject files claimed by a wheel and another package.
/// Run before clobber resolution: moving an overwritten file cannot recover
/// its original contents.
pub(super) fn validate_wheel_ownership(
    records: &[PrefixRecord],
    ignored: Option<&HashSet<PackageName>>,
) -> io::Result<()> {
    let owners: HashMap<_, _> = records
        .iter()
        .enumerate()
        .filter(|(_, record)| is_managed_wheel(&record.repodata_record, ignored))
        .flat_map(|(index, record)| {
            record
                .paths_data
                .paths
                .iter()
                .filter(|entry| entry.path_type != PathType::Directory)
                .map(move |entry| (entry.relative_path.as_path(), index))
        })
        .collect();
    if owners.is_empty() {
        return Ok(());
    }
    for (index, record) in records.iter().enumerate() {
        for entry in record
            .paths_data
            .paths
            .iter()
            .filter(|entry| entry.path_type != PathType::Directory)
        {
            if let Some(&owner) = owners.get(entry.relative_path.as_path())
                && owner != index
            {
                return Err(invalid(format!(
                    "ambiguous wheel file ownership at {}: {} and {}; reinstall the conflicting packages",
                    entry.relative_path.display(),
                    records[owner].file_name(),
                    record.file_name()
                )));
            }
        }
    }
    Ok(())
}

// PEP 503 normalization, matching the cache converter's identity check.
fn normalize_name(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    for character in name.chars() {
        if matches!(character, '-' | '_' | '.') {
            if !normalized.ends_with('-') {
                normalized.push('-');
            }
        } else {
            normalized.push(character.to_ascii_lowercase());
        }
    }
    normalized
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn validate_relative(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || path.starts_with("conda-meta")
    {
        return Err(invalid(format!(
            "unsafe wheel inventory path: {}",
            path.display()
        )));
    }
    Ok(())
}

fn posix_path(path: &Path) -> io::Result<String> {
    path.components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .ok_or_else(|| invalid("non-UTF-8 wheel path"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("/"))
}
