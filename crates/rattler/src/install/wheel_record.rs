//! Destination-aware RECORD generation for converted wheels.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{self, Write},
    path::{Component, Path},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rattler_conda_types::{
    PackageName, PrefixRecord, package::DistArchiveType, prefix::Prefix, prefix_record::PathType,
};
use rattler_digest::{Sha256, compute_bytes_digest, compute_file_digest};

#[cfg(test)]
mod tests;

/// Finalize only records supplied by the transaction, never discover files by
/// scanning site-packages. Reload metadata after clobber and script finalization.
pub(super) fn finalize_wheel_records(
    prefix: &Prefix,
    records: &mut [PrefixRecord],
    ignored: Option<&HashSet<PackageName>>,
    rehash: bool,
) -> io::Result<()> {
    if !records
        .iter()
        .any(|record| is_managed_wheel(record, ignored))
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
        .filter(|record| is_managed_wheel(record, ignored))
    {
        let metadata_path = prefix.join("conda-meta").join(record.file_name());
        // Match the identity validated by the converter, not a guessed Python
        // layout. Existing paths can outlive Python or remain in an older layout.
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
        let [record_index] = candidates.as_slice() else {
            return Err(invalid(format!(
                "expected one unambiguous wheel RECORD for {}",
                record.file_name()
            )));
        };
        let record_index = *record_index;
        let record_path = record.paths_data.paths[record_index].relative_path.clone();
        validate_relative(&record_path)?;
        let base = record_path
            .parent()
            .and_then(Path::parent)
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
            let (hash, size) = if index == record_index {
                (String::new(), String::new())
            } else {
                let path = prefix.join(&entry.relative_path);
                let metadata = fs_err::metadata(&path)?;
                if !metadata.is_file() {
                    return Err(invalid(format!(
                        "wheel payload is not a file: {}",
                        path.display()
                    )));
                }
                let known_hash = entry.sha256_in_prefix.or(entry.sha256);
                // Linking supplies hashes of the final bytes. Post-link hooks
                // invalidate that assumption.
                let hash = match known_hash {
                    Some(hash) if !rehash && entry.size_in_bytes == Some(metadata.len()) => hash,
                    _ => compute_file_digest::<Sha256>(&path)?,
                };
                let installed_hash = (entry.sha256 != Some(hash)).then_some(hash);
                if entry.sha256_in_prefix != installed_hash
                    || entry.size_in_bytes != Some(metadata.len())
                {
                    entry.sha256_in_prefix = installed_hash;
                    entry.size_in_bytes = Some(metadata.len());
                    metadata_changed = true;
                }
                (
                    format!("sha256={}", URL_SAFE_NO_PAD.encode(hash)),
                    metadata.len().to_string(),
                )
            };
            if rows.insert(relative, (hash, size)).is_some() {
                return Err(invalid("duplicate owned path in wheel RECORD"));
            }
        }
        let mut writer = csv::WriterBuilder::new()
            .has_headers(false)
            .from_writer(Vec::new());
        for (path, (hash, size)) in rows {
            writer
                .write_record([path, hash, size])
                .map_err(io::Error::other)?;
        }
        let bytes = writer.into_inner().map_err(io::Error::other)?;
        let destination = prefix.join(&record_path);
        // Reject directory symlinks: replacing a file through a symlinked
        // dist-info directory could modify the cache itself. A file symlink is
        // safe to replace atomically and is deliberately not canonicalized.
        let mut parent = prefix.path().to_path_buf();
        for component in record_path
            .parent()
            .expect("validated RECORD path")
            .components()
        {
            parent.push(component);
            if fs_err::symlink_metadata(&parent)?.file_type().is_symlink() {
                return Err(invalid(format!(
                    "RECORD parent is a symlink: {}",
                    parent.display()
                )));
            }
        }
        if fs_err::read(&destination)? != bytes {
            let metadata = fs_err::metadata(&destination)?;
            let mut temporary =
                tempfile::NamedTempFile::new_in(destination.parent().expect("RECORD has parent"))?;
            temporary.write_all(&bytes)?;
            filetime::set_file_mtime(
                temporary.path(),
                filetime::FileTime::from_last_modification_time(&metadata),
            )?;
            temporary
                .as_file()
                .set_permissions(metadata.permissions())?;
            temporary
                .persist(&destination)
                .map_err(|error| error.error)?;
        }
        let entry = &mut record.paths_data.paths[record_index];
        let hash = compute_bytes_digest::<Sha256>(&bytes);
        let installed_hash = (entry.sha256 != Some(hash)).then_some(hash);
        // Reconcile even if a prior attempt replaced RECORD but failed while
        // writing conda-meta. This makes no-op retries recover that failure.
        if entry.sha256_in_prefix != installed_hash
            || entry.size_in_bytes != Some(bytes.len() as u64)
        {
            entry.sha256_in_prefix = installed_hash;
            entry.size_in_bytes = Some(bytes.len() as u64);
            metadata_changed = true;
        }
        if metadata_changed {
            record.write_to_path(metadata_path, true)?;
        }
    }
    Ok(())
}

fn is_managed_wheel(record: &PrefixRecord, ignored: Option<&HashSet<PackageName>>) -> bool {
    matches!(
        record.repodata_record.identifier.archive_type,
        DistArchiveType::Wheel(_)
    ) && ignored
        .is_none_or(|ignored| !ignored.contains(&record.repodata_record.package_record.name))
}

/// Legacy installers could overwrite generated commands without recording a
/// clobber. Do not manufacture a RECORD that claims another package's file.
/// Check before clobber resolution as well: an already-overwritten command
/// cannot be recovered by moving its surviving bytes to a different owner.
pub(super) fn validate_wheel_ownership(
    records: &[PrefixRecord],
    ignored: Option<&HashSet<PackageName>>,
) -> io::Result<()> {
    let owners: HashMap<_, _> = records
        .iter()
        .enumerate()
        .filter(|(_, record)| is_managed_wheel(record, ignored))
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
