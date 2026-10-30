//! RECORD generation for the cache layout, not final installed paths.

use std::{io, path::Path};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rattler_conda_types::package::PathsEntry;

use super::SITE_PACKAGES;
use crate::ExtractError;

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
