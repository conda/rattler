//! Converts wheels into conda cache entries using channel-provided metadata.
//!
//! Libraries go into `site-packages/`, scripts into `python-scripts/`, headers
//! into `include/`, and data into the package root. The conda linker handles
//! noarch path remapping and entry points from the generated `info/link.json`.
//! Headers retain conda-pypi's `include/` mapping; pip's Python-version-specific
//! header paths depend on the target environment, which is unknown during caching.
//! Declared license files are also copied into `info/licenses/` (CEP 34).
//! Raw `#!python` and `#!pythonw` scripts use relocatable shebangs targeting
//! `bin/python` on Unix. Windows launchers for raw scripts are not generated.
//!
//! `RECORD` describes the cache layout, not final installed paths. The full
//! `rattler::install::Installer` finalizes an environment-specific copy after
//! linking; low-level linking alone does not. Original RECORD signatures are
//! discarded. Original RECORD hashes are not verified; callers
//! should verify the returned archive hashes against trusted repodata when available.

use std::{
    collections::HashSet,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use rattler_conda_types::{
    NoArchType, PackageRecord,
    package::{
        EntryPoint, FileMode, IndexJson, LinkJson, NoArchLinks, PackageFile, ParseEntryPointError,
        PathType, PathsEntry, PathsJson, PrefixPlaceholder, PythonEntryPoints,
    },
};
use rattler_digest::{HashingReader, Sha256};
use serde::{Deserialize, Serialize};
use tempfile::SpooledTempFile;
use zip::ZipArchive;

use crate::{ExtractError, ExtractResult};

mod licenses;
mod record;
#[cfg(test)]
mod tests;

const SITE_PACKAGES: &str = "site-packages";
const PYTHON_SCRIPTS: &str = "python-scripts";
const INSTALLER: &str = "conda\n";
const SCRIPT_PREFIX_PLACEHOLDER: &str = "/__rattler_wheel_prefix_placeholder__";

/// Wheels can be spooled to memory up to this size before spilling to disk.
const SPOOL_LIMIT: usize = 5 * 1024 * 1024;

/// Bump when the converted layout changes to invalidate older cache entries.
pub const CONVERSION_VERSION: u64 = 2;

/// A wheel cannot be represented safely in the conda cache layout.
#[derive(Debug, thiserror::Error)]
pub enum WheelError {
    /// Missing or malformed wheel metadata.
    #[error("invalid wheel metadata: {0}")]
    Metadata(String),
    /// The distribution metadata does not match the channel record.
    #[error("wheel {dist_info} does not match package {expected}")]
    IdentityMismatch {
        /// The wheel's distribution metadata directory.
        dist_info: String,
        /// The name and version from the channel record.
        expected: String,
    },
    /// Generic noarch packages do not receive Python path remapping.
    #[error("wheels require Python noarch path remapping, not noarch: generic")]
    GenericNoArch,
    /// Two files, or a file and a directory, share an output path.
    #[error("wheel destination collision: {0}")]
    DestinationCollision(PathBuf),
    /// An archive contains multiple files with the same path.
    #[error("duplicate wheel archive path: {0}")]
    DuplicatePath(PathBuf),
    /// A wheel payload would overwrite generated conda metadata.
    #[error("wheel payload overlaps conda metadata: {0}")]
    ReservedPath(PathBuf),
    /// A wheel uses an unrecognized installation scheme.
    #[error("unknown wheel .data scheme '{scheme}' for {path}")]
    UnsupportedScheme {
        /// The unknown scheme name.
        scheme: String,
        /// The original archive path.
        path: PathBuf,
    },
    /// An entry point cannot be parsed by the conda linker.
    #[error(transparent)]
    EntryPoint(#[from] ParseEntryPointError),
}

/// Conversion metadata stored in `info/wheel.json` for cache invalidation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WheelJson {
    /// The [`CONVERSION_VERSION`] of the converter that wrote this entry.
    pub conversion_version: u64,
}

impl PackageFile for WheelJson {
    fn package_path() -> &'static Path {
        Path::new("info/wheel.json")
    }

    fn from_str(str: &str) -> Result<Self, io::Error> {
        serde_json::from_str(str).map_err(Into::into)
    }
}

/// Converts a wheel into conda layout using `record` for package metadata.
/// Returns the original archive's hashes and size.
pub fn extract_wheel(
    reader: impl Read,
    destination: &Path,
    record: &PackageRecord,
) -> Result<ExtractResult, ExtractError> {
    crate::read::process_with_hashing(reader, |reader| {
        extract_wheel_without_hashing(reader, destination, record)
    })
}

/// Same as [`extract_wheel`] but without computing the archive hashes.
pub(crate) fn extract_wheel_without_hashing(
    mut reader: impl Read,
    destination: &Path,
    record: &PackageRecord,
) -> Result<(), ExtractError> {
    // Zip archives need random access, so buffer the stream first.
    let mut temp_file = SpooledTempFile::new(SPOOL_LIMIT);
    io::copy(&mut reader, &mut temp_file)?;
    temp_file.seek(SeekFrom::Start(0))?;
    extract_wheel_from_seek(temp_file, destination, record)
}

/// Converts a seekable wheel without copying or hashing the archive.
pub(crate) fn extract_wheel_from_seek(
    reader: impl Read + Seek,
    destination: &Path,
    record: &PackageRecord,
) -> Result<(), ExtractError> {
    let mut archive = ZipArchive::new(reader)?;
    if record.noarch.is_generic() {
        return Err(WheelError::GenericNoArch.into());
    }
    let layout = WheelLayout::detect(&mut archive, record)?;
    let WheelPlan {
        entries,
        entry_points,
        licenses,
    } = layout.plan(&mut archive)?;
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;
    let mut paths = Vec::with_capacity(entries.len() + 2);

    for PlannedEntry { index, target } in entries {
        let mut source = archive.by_index(index)?;
        let mode = source.unix_mode();

        let entry = if Path::new(source.name()?.as_ref()).starts_with(layout.data.join("scripts")) {
            write_script_entry(destination, target, &mut source, mode)?
        } else {
            write_entry(destination, target, &mut source, mode)?
        };
        paths.push(entry);
    }

    for PlannedEntry { index, target } in licenses {
        // Package metadata is not linked into the environment or listed in RECORD.
        write_entry(destination, target, archive.by_index(index)?, None)?;
    }

    let installer_path = Path::new(SITE_PACKAGES)
        .join(&layout.dist_info)
        .join("INSTALLER");
    paths.push(write_entry(
        destination,
        installer_path,
        INSTALLER.as_bytes(),
        None,
    )?);

    paths.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    let record_bytes = record::for_cache(&paths, &layout.dist_info)?;
    paths.push(write_entry(
        destination,
        Path::new(SITE_PACKAGES)
            .join(&layout.dist_info)
            .join("RECORD"),
        record_bytes.as_slice(),
        None,
    )?);
    paths.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    write_info(destination, record, paths, entry_points)
}

/// An archive member and its validated destination.
struct PlannedEntry {
    index: usize,
    target: PathBuf,
}

/// Validated payload destinations and linker-generated scripts.
struct WheelPlan {
    entries: Vec<PlannedEntry>,
    entry_points: Vec<EntryPoint>,
    licenses: Vec<PlannedEntry>,
}

/// Reserves file and directory paths to reject clobbers before extraction.
#[derive(Default)]
struct Destinations {
    files: HashSet<PathBuf>,
    directories: HashSet<PathBuf>,
}

impl Destinations {
    fn insert_payload(&mut self, path: &Path) -> Result<(), ExtractError> {
        self.insert(path)?;
        if let Ok(script) = path.strip_prefix(PYTHON_SCRIPTS) {
            // Noarch Python scripts can be linked on either platform.
            self.insert(&Path::new("bin").join(script))?;
            self.insert(&Path::new("Scripts").join(script))?;
        }
        Ok(())
    }

    fn insert(&mut self, path: &Path) -> Result<(), ExtractError> {
        if self.files.contains(path) || self.directories.contains(path) {
            return Err(WheelError::DestinationCollision(path.to_path_buf()).into());
        }
        for parent in path
            .ancestors()
            .skip(1)
            .filter(|p| !p.as_os_str().is_empty())
        {
            if self.files.contains(parent) {
                return Err(WheelError::DestinationCollision(parent.to_path_buf()).into());
            }
            self.directories.insert(parent.to_path_buf());
        }
        self.files.insert(path.to_path_buf());
        Ok(())
    }
}

/// The names of the two special directories a wheel may contain.
struct WheelLayout {
    /// `{name}-{version}.dist-info`
    dist_info: PathBuf,
    /// `{name}-{version}.data`
    data: PathBuf,
}

impl WheelLayout {
    fn detect<R: Read + Seek>(
        archive: &mut ZipArchive<R>,
        record: &PackageRecord,
    ) -> Result<Self, ExtractError> {
        let mut dist_info: Option<String> = None;
        for name in archive.file_names() {
            let name = name?;
            let Some((first, _)) = name.split_once('/') else {
                continue;
            };
            let Some(stem) = first.strip_suffix(".dist-info") else {
                continue;
            };
            match &dist_info {
                Some(existing) if existing != stem => {
                    return Err(WheelError::Metadata(format!(
                        "multiple .dist-info directories: {existing}.dist-info and {first}"
                    ))
                    .into());
                }
                Some(_) => {}
                None => dist_info = Some(stem.to_owned()),
            }
        }
        let Some(stem) = dist_info else {
            return Err(WheelError::Metadata("no .dist-info directory".into()).into());
        };
        let (name, version) = stem.rsplit_once('-').ok_or_else(|| {
            WheelError::Metadata(format!("invalid .dist-info directory: {stem}.dist-info"))
        })?;
        let version = pep440_rs::Version::from_str(version)
            .map_err(|err| WheelError::Metadata(format!("invalid wheel version: {err}")))?;
        let expected_version = pep440_rs::Version::from_str(&record.version.to_string())
            .map_err(|err| WheelError::Metadata(format!("invalid record version: {err}")))?;
        if normalize_name(name) != normalize_name(record.name.as_source())
            || version != expected_version
        {
            return Err(WheelError::IdentityMismatch {
                dist_info: format!("{stem}.dist-info"),
                expected: format!("{} {}", record.name.as_source(), record.version),
            }
            .into());
        }
        let Some(wheel_index) = archive.index_for_name(&format!("{stem}.dist-info/WHEEL")) else {
            return Err(WheelError::Metadata(format!("{stem}.dist-info/WHEEL is missing")).into());
        };
        let mut wheel_txt = String::new();
        archive
            .by_index(wheel_index)?
            .read_to_string(&mut wheel_txt)?;
        let mut versions = wheel_txt
            .lines()
            .filter_map(|line| line.split_once(':'))
            .filter(|(key, _)| key.trim().eq_ignore_ascii_case("Wheel-Version"))
            .map(|(_, value)| value.trim());
        let (Some(version), None) = (versions.next(), versions.next()) else {
            return Err(WheelError::Metadata(
                "WHEEL must contain exactly one Wheel-Version".into(),
            )
            .into());
        };
        let (major, minor) = version
            .split_once('.')
            .filter(|(major, minor)| {
                !major.is_empty()
                    && !minor.is_empty()
                    && major.bytes().all(|b| b.is_ascii_digit())
                    && minor.bytes().all(|b| b.is_ascii_digit())
            })
            .and_then(|(major, minor)| {
                Some((major.parse::<u64>().ok()?, minor.parse::<u64>().ok()?))
            })
            .ok_or_else(|| WheelError::Metadata(format!("invalid Wheel-Version: {version}")))?;
        if major != 1 {
            return Err(
                WheelError::Metadata(format!("unsupported Wheel-Version: {version}")).into(),
            );
        }
        if minor > 0 {
            tracing::warn!(
                wheel_version = version,
                "wheel version is newer than supported version 1.0"
            );
        }
        Ok(Self {
            dist_info: PathBuf::from(format!("{stem}.dist-info")),
            data: PathBuf::from(format!("{stem}.data")),
        })
    }

    /// Reject conflicting destinations before writing files.
    fn plan<R: Read + Seek>(&self, archive: &mut ZipArchive<R>) -> Result<WheelPlan, ExtractError> {
        let mut entry_points = Vec::new();
        if let Some(index) =
            archive.index_for_name(&format!("{}/entry_points.txt", self.dist_info.display()))
        {
            let mut text = String::new();
            archive.by_index(index)?.read_to_string(&mut text)?;
            entry_points = parse_entry_points(&text)?;
        }
        let mut destinations = Destinations::default();
        for entry_point in &entry_points {
            for name in [
                entry_point.command.clone(),
                format!("{}.exe", entry_point.command),
                format!("{}-script.py", entry_point.command),
            ] {
                destinations.insert_payload(&Path::new(PYTHON_SCRIPTS).join(name))?;
            }
        }
        for name in ["INSTALLER", "RECORD"] {
            destinations.insert(&Path::new(SITE_PACKAGES).join(&self.dist_info).join(name))?;
        }
        let mut sources = HashSet::new();
        let mut plan = Vec::with_capacity(archive.len());
        for index in 0..archive.len() {
            let file = archive.by_index(index)?;
            if file.is_dir() {
                continue;
            }
            let name = file.name()?;
            // Reject ambiguous POSIX paths before normalization.
            if name.contains('\\') || name.split('/').any(|part| matches!(part, "" | "." | "..")) {
                return Err(ExtractError::InvalidArchivePath(PathBuf::from(
                    name.as_ref(),
                )));
            }
            let archive_path = file
                .enclosed_name()
                .ok_or_else(|| ExtractError::InvalidArchivePath(PathBuf::from(name.as_ref())))?;
            if !sources.insert(archive_path.clone()) {
                return Err(WheelError::DuplicatePath(archive_path).into());
            }
            let Some(target) = self.target_path(&archive_path)? else {
                continue;
            };
            if target.components().next().is_some_and(|component| {
                let root = component.as_os_str().to_string_lossy();
                root.eq_ignore_ascii_case("info") || root.eq_ignore_ascii_case("conda-meta")
            }) {
                return Err(WheelError::ReservedPath(target).into());
            }
            destinations.insert_payload(&target)?;
            plan.push(PlannedEntry { index, target });
        }
        Ok(WheelPlan {
            entries: plan,
            entry_points,
            licenses: licenses::plan(archive, &self.dist_info, &mut destinations)?,
        })
    }

    fn is_dist_info_file(&self, archive_path: &Path, file_name: &str) -> bool {
        archive_path == self.dist_info.join(file_name)
    }

    /// Maps wheel paths to conda paths, skipping replaced or discarded metadata.
    fn target_path(&self, archive_path: &Path) -> Result<Option<PathBuf>, ExtractError> {
        if archive_path
            .components()
            .any(|component| component.as_os_str() == "__pycache__")
        {
            return Ok(None);
        }
        if ["INSTALLER", "RECORD", "RECORD.jws", "RECORD.p7s"]
            .iter()
            .any(|name| self.is_dist_info_file(archive_path, name))
        {
            return Ok(None);
        }

        let Ok(rest) = archive_path.strip_prefix(&self.data) else {
            return Ok(Some(Path::new(SITE_PACKAGES).join(archive_path)));
        };

        let mut components = rest.components();
        let scheme = match components.next() {
            Some(Component::Normal(scheme)) => scheme.to_string_lossy().into_owned(),
            _ => {
                return Err(ExtractError::InvalidArchivePath(archive_path.to_path_buf()));
            }
        };
        let rest: PathBuf = components.collect();
        if rest.as_os_str().is_empty() {
            return Err(ExtractError::InvalidArchivePath(archive_path.to_path_buf()));
        }

        let target = match scheme.as_str() {
            "purelib" | "platlib" => Path::new(SITE_PACKAGES).join(rest),
            "scripts" => Path::new(PYTHON_SCRIPTS).join(rest),
            "headers" => Path::new("include").join(rest),
            "data" => rest,
            other => {
                return Err(WheelError::UnsupportedScheme {
                    scheme: other.to_owned(),
                    path: archive_path.to_path_buf(),
                }
                .into());
            }
        };
        Ok(Some(target))
    }
}

/// PEP 503 names are case-insensitive and collapse runs of `-`, `_`, and `.`.
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

/// Rewrites wheel Python shebangs to conda's relocatable Unix script layout.
fn write_script_entry(
    destination: &Path,
    relative_path: PathBuf,
    mut content: impl Read,
    unix_mode: Option<u32>,
) -> Result<PathsEntry, ExtractError> {
    let mut prefix = Vec::with_capacity(8);
    content.by_ref().take(8).read_to_end(&mut prefix)?;
    if prefix != b"#!python" {
        return write_entry(
            destination,
            relative_path,
            prefix.as_slice().chain(content),
            unix_mode,
        );
    }

    // Like PyPA installer, replace the entire first line, including CRLF and
    // any suffix. On Unix pythonw uses the same interpreter as python.
    let mut body = BufReader::new(content);
    body.skip_until(b'\n')?;
    let shebang = format!("#!{SCRIPT_PREFIX_PLACEHOLDER}/bin/python\n");
    let mut entry = write_entry(
        destination,
        relative_path,
        shebang.as_bytes().chain(body),
        Some(unix_mode.unwrap_or_default() | 0o111),
    )?;
    entry.prefix_placeholder = Some(PrefixPlaceholder {
        file_mode: FileMode::Text,
        placeholder: SCRIPT_PREFIX_PLACEHOLDER.to_owned(),
        experimental_offsets: None,
    });
    Ok(entry)
}

/// Writes a file and computes its `paths.json` entry.
fn write_entry(
    destination: &Path,
    relative_path: PathBuf,
    mut content: impl Read,
    unix_mode: Option<u32>,
) -> Result<PathsEntry, ExtractError> {
    let full_path = destination.join(&relative_path);
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if unix_mode.is_some_and(|mode| mode & 0o111 != 0) {
            0o777
        } else {
            0o666
        });
    }
    #[cfg(not(unix))]
    let _ = unix_mode;
    let mut file = options.open(&full_path)?;
    let mut hashing = HashingReader::<_, Sha256>::new(&mut content);
    let size_in_bytes = io::copy(&mut hashing, &mut file)?;
    let (_, sha256) = hashing.finalize();

    Ok(PathsEntry {
        relative_path,
        no_link: false,
        path_type: PathType::HardLink,
        prefix_placeholder: None,
        sha256: Some(sha256),
        size_in_bytes: Some(size_in_bytes),
    })
}

/// Reads console and GUI entry points from `entry_points.txt`.
fn parse_entry_points(entry_points_txt: &str) -> Result<Vec<EntryPoint>, ExtractError> {
    let mut entry_points = Vec::new();
    let mut in_scripts_section = false;
    for line in entry_points_txt.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_scripts_section = matches!(section.trim(), "console_scripts" | "gui_scripts");
            continue;
        }
        if !in_scripts_section {
            continue;
        }
        // `name = module:function [extra1, extra2]`: extras do not apply here.
        let spec = line.split_once('[').map_or(line, |(spec, _)| spec).trim();
        let entry_point = EntryPoint::from_str(spec).map_err(WheelError::from)?;
        entry_points.push(entry_point);
    }
    Ok(entry_points)
}

/// Writes the generated conda metadata.
fn write_info(
    destination: &Path,
    record: &PackageRecord,
    paths: Vec<PathsEntry>,
    entry_points: Vec<EntryPoint>,
) -> Result<(), ExtractError> {
    let index_json = IndexJson {
        arch: None,
        build: record.build.clone(),
        build_number: record.build_number,
        constrains: record.constrains.clone(),
        depends: record.depends.clone(),
        extra_depends: record.extra_depends.clone(),
        features: record.features.clone(),
        flags: record.flags.clone(),
        license: record.license.clone(),
        license_family: record.license_family.clone(),
        name: record.name.clone(),
        // Wheel layout uses noarch Python path remapping.
        noarch: NoArchType::python(),
        platform: None,
        purls: record.purls.clone(),
        python_site_packages_path: None,
        repodata_revision: None,
        subdir: Some(record.subdir.clone()),
        timestamp: record.timestamp,
        track_features: record.track_features.clone(),
        version: record.version.clone(),
    };
    let paths_json = PathsJson {
        paths,
        paths_version: 1,
    };
    let link_json = LinkJson {
        noarch: NoArchLinks::Python(PythonEntryPoints { entry_points }),
        package_metadata_version: 1,
    };
    let wheel_json = WheelJson {
        conversion_version: CONVERSION_VERSION,
    };

    write_json(destination, IndexJson::package_path(), &index_json)?;
    write_json(destination, PathsJson::package_path(), &paths_json)?;
    write_json(destination, LinkJson::package_path(), &link_json)?;
    write_json(destination, WheelJson::package_path(), &wheel_json)?;
    Ok(())
}

fn write_json<T: serde::Serialize>(
    destination: &Path,
    relative_path: &Path,
    value: &T,
) -> Result<(), ExtractError> {
    let full_path = destination.join(relative_path);
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create_new(full_path)?;
    serde_json::to_writer_pretty(file, value).map_err(io::Error::other)?;
    Ok(())
}
