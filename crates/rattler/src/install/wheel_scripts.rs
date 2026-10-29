//! Finalize raw wheel GUI scripts once the environment's files are in place.

use std::{
    io::{self, BufRead, BufReader, Write},
    path::Path,
};

use fs_err::File;
use rattler_conda_types::{PrefixRecord, package::DistArchiveType, prefix::Prefix};
use rattler_digest::{Sha256, compute_file_digest};
use rattler_package_streaming::wheel::PYTHON_GUI_SCRIPT_PREFIX_PLACEHOLDER;

/// Revisit only newly installed wheel records, never arbitrary files in Scripts/.
/// Clobber resolution may have moved scripts, so reload their final recorded paths.
pub(super) fn finalize_windows_wheel_scripts<'a>(
    prefix: &Prefix,
    records: impl Iterator<Item = &'a PrefixRecord>,
) -> io::Result<()> {
    let executable = if prefix.join("pythonw.exe").is_file() {
        "pythonw.exe"
    } else {
        "python.exe"
    };
    for record in records {
        if !matches!(
            record.repodata_record.identifier.archive_type,
            DistArchiveType::Wheel(_)
        ) || !record.paths_data.paths.iter().any(|entry| {
            entry.prefix_placeholder.as_deref() == Some(PYTHON_GUI_SCRIPT_PREFIX_PLACEHOLDER)
        }) {
            continue;
        }
        let metadata_path = prefix.join("conda-meta").join(record.file_name());
        let mut record = PrefixRecord::from_path(&metadata_path)?;
        let mut changed = false;
        for entry in &mut record.paths_data.paths {
            if entry.prefix_placeholder.as_deref() != Some(PYTHON_GUI_SCRIPT_PREFIX_PLACEHOLDER) {
                continue;
            }
            let path = prefix.join(&entry.relative_path);
            if rewrite_interpreter(&path, executable)? {
                entry.sha256_in_prefix = Some(compute_file_digest::<Sha256>(&path)?);
                entry.size_in_bytes = Some(fs_err::metadata(&path)?.len());
                changed = true;
            }
        }
        if changed {
            record.write_to_path(metadata_path, true)?;
        }
    }
    Ok(())
}

fn rewrite_interpreter(path: &Path, executable: &str) -> io::Result<bool> {
    let metadata = fs_err::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::other(format!(
            "wheel script is not a regular file: {}",
            path.display()
        )));
    }
    let mut source = BufReader::new(File::open(path)?);
    let mut header = Vec::new();
    source.read_until(b'\n', &mut header)?;
    // Linking already relocated the prefix (including alternative target
    // prefixes). Preserve it, changing only the interpreter selected at that time.
    let prefix = header
        .strip_suffix(b"/python.exe\r\n")
        .or_else(|| header.strip_suffix(b"/pythonw.exe\r\n"))
        .filter(|prefix| prefix.starts_with(b"#!"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected wheel script shebang: {}", path.display()),
            )
        })?;
    let mut desired = prefix.to_vec();
    write!(desired, "/{executable}\r\n")?;
    if header == desired {
        return Ok(false);
    }

    // Replace rather than truncate: even a hardlinked installed script must
    // never mutate another environment or the shared package cache.
    let mut replacement =
        tempfile::NamedTempFile::new_in(path.parent().expect("script has parent"))?;
    replacement.write_all(&desired)?;
    io::copy(&mut source, &mut replacement)?;
    filetime::set_file_mtime(
        replacement.path(),
        filetime::FileTime::from_last_modification_time(&metadata),
    )?;
    replacement
        .as_file()
        .set_permissions(metadata.permissions())?;
    drop(source);
    replacement.persist(path).map_err(|error| error.error)?;
    Ok(true)
}
