//! Copies declared wheel licenses into conda package metadata (CEP 34).

use std::{
    collections::HashSet,
    io::{Read, Seek},
    path::{Path, PathBuf},
};

use zip::ZipArchive;

use super::{Destinations, PlannedEntry};
use crate::ExtractError;

/// Resolve licenses only within this wheel's `.dist-info`, preserving listed paths.
pub(super) fn plan<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    dist_info: &Path,
    destinations: &mut Destinations,
) -> Result<Vec<PlannedEntry>, ExtractError> {
    let Some(metadata_index) = archive.index_for_name(&format!("{}/METADATA", dist_info.display()))
    else {
        return Ok(Vec::new());
    };
    let mut metadata = String::new();
    archive
        .by_index(metadata_index)?
        .read_to_string(&mut metadata)?;
    let mut licenses = Vec::new();
    let mut seen = HashSet::new();
    for entry in license_files(&metadata) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        // Validate POSIX paths independently of the host OS (including Windows drives/ADS).
        if entry.contains(['\\', ':', '\0'])
            || entry.split('/').any(|part| matches!(part, "" | "." | ".."))
        {
            return Err(ExtractError::InvalidArchivePath(PathBuf::from(entry)));
        }
        let mut found = false;
        // Match conda-pypi: older flat layouts first, then PEP 639's licenses tree.
        for source in [
            format!("{}/{entry}", dist_info.display()),
            format!("{}/licenses/{entry}", dist_info.display()),
        ] {
            let Some(index) = archive.index_for_name(&source) else {
                continue;
            };
            if archive.by_index(index)?.is_dir() {
                continue;
            }
            found = true;
            if seen.insert(index) {
                let target = Path::new("info/licenses").join(entry);
                destinations.insert(&target)?;
                licenses.push(PlannedEntry { index, target });
            }
            break;
        }
        if !found {
            tracing::warn!(license_file = entry, dist_info = %dist_info.display(), "declared license file is missing from wheel metadata");
        }
    }
    Ok(licenses)
}

/// Read repeated, optionally folded headers, stopping before the description body.
fn license_files(metadata: &str) -> Vec<String> {
    let mut entries: Vec<String> = Vec::new();
    let mut in_license = false;
    for line in metadata.lines().take_while(|line| !line.is_empty()) {
        if line.starts_with([' ', '\t']) {
            if in_license {
                let entry = entries.last_mut().expect("license header was added");
                entry.push(' ');
                entry.push_str(line.trim());
            }
            continue;
        }
        in_license = false;
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("License-File")
        {
            entries.push(value.trim().to_owned());
            in_license = true;
        }
    }
    entries
}
