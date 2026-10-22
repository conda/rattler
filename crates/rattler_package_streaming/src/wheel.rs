//! Converts wheels into conda cache entries using channel-provided metadata.
//!
//! Libraries go into `site-packages/`, scripts into `python-scripts/`, headers
//! into `include/`, and data into the package root. The conda linker handles
//! noarch path remapping and entry points from the generated `info/link.json`.
//!
//! `RECORD` describes the cache layout, not final installed paths; its original
//! signatures are discarded. Raw script shebangs are preserved (see RAT-288).

use std::{
    collections::HashSet,
    io::{self, Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use rattler_conda_types::{
    NoArchType, PackageRecord,
    package::{
        EntryPoint, IndexJson, LinkJson, NoArchLinks, PackageFile, PathType, PathsEntry, PathsJson,
        PythonEntryPoints,
    },
};
use rattler_digest::{HashingReader, Sha256};
use serde::{Deserialize, Serialize};
use tempfile::SpooledTempFile;
use zip::ZipArchive;

use crate::{ExtractError, ExtractResult};

mod record;
use record::RecordManifest;

const SITE_PACKAGES: &str = "site-packages";
const PYTHON_SCRIPTS: &str = "python-scripts";
const INSTALLER: &str = "conda\n";

/// Wheels can be spooled to memory up to this size before spilling to disk.
const SPOOL_LIMIT: usize = 5 * 1024 * 1024;

/// Bump when the converted layout changes to invalidate older cache entries.
pub const CONVERSION_VERSION: u64 = 2;

/// Conversion metadata stored in `info/wheel.json` for cache invalidation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WheelJson {
    /// The [`CONVERSION_VERSION`] of the converter that wrote this entry.
    pub conversion_version: u64,

    /// The wheel's `.dist-info` directory name.
    pub dist_info: String,

    /// Compatibility tags from the `WHEEL` file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// Archive SHA-256 from repodata, encoded as hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
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
    let mut archive = ZipArchive::new(temp_file)?;

    let layout = WheelLayout::detect(&mut archive)?;
    let plan = layout.plan(&mut archive)?;
    let manifest = RecordManifest::read(&mut archive, &layout.dist_info)?;
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;
    let mut paths = Vec::with_capacity(plan.len() + 2);
    let mut entry_points = Vec::new();

    for PlannedEntry {
        index,
        archive_path,
        target,
    } in plan
    {
        let file = archive.by_index(index)?;
        let mode = file.unix_mode();
        let mut source = manifest.reader(&archive_path, file);
        let Some(target) = target else {
            // Validate even files that are replaced or discarded.
            io::copy(&mut source, &mut io::sink())?;
            source.verify()?;
            continue;
        };

        let entry = if layout.is_dist_info_file(&archive_path, "entry_points.txt") {
            let mut text = String::new();
            source.read_to_string(&mut text)?;
            source.verify()?;
            entry_points = parse_entry_points(&text)?;
            write_entry(destination, target, text.as_bytes(), mode)?
        } else {
            let entry = write_entry(destination, target, &mut source, mode)?;
            source.verify()?;
            entry
        };
        paths.push(entry);
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
    write_info(destination, record, &layout, paths, entry_points)
}

struct PlannedEntry {
    index: usize,
    archive_path: PathBuf,
    target: Option<PathBuf>,
}

#[derive(Default)]
struct Destinations {
    files: HashSet<PathBuf>,
    directories: HashSet<PathBuf>,
}

impl Destinations {
    fn insert(&mut self, path: &Path) -> Result<(), ExtractError> {
        if self.files.contains(path) || self.directories.contains(path) {
            return Err(ExtractError::InvalidWheel(format!(
                "destination collision: {}",
                path.display()
            )));
        }
        for parent in path
            .ancestors()
            .skip(1)
            .filter(|p| !p.as_os_str().is_empty())
        {
            if self.files.contains(parent) {
                return Err(ExtractError::InvalidWheel(format!(
                    "file/directory collision: {}",
                    parent.display()
                )));
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
    /// The `Tag:` values of the `WHEEL` file.
    tags: Vec<String>,
}

impl WheelLayout {
    fn detect<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<Self, ExtractError> {
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
                    return Err(ExtractError::InvalidWheel(format!(
                        "multiple .dist-info directories: {existing}.dist-info and {first}"
                    )));
                }
                Some(_) => {}
                None => dist_info = Some(stem.to_owned()),
            }
        }
        let Some(stem) = dist_info else {
            return Err(ExtractError::InvalidWheel(
                "no .dist-info directory".to_string(),
            ));
        };
        let Some(wheel_index) = archive.index_for_name(&format!("{stem}.dist-info/WHEEL")) else {
            return Err(ExtractError::InvalidWheel(format!(
                "{stem}.dist-info/WHEEL is missing"
            )));
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
            return Err(ExtractError::InvalidWheel(
                "WHEEL must contain exactly one Wheel-Version".into(),
            ));
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
            .ok_or_else(|| {
                ExtractError::InvalidWheel(format!("invalid Wheel-Version: {version}"))
            })?;
        if major != 1 {
            return Err(ExtractError::InvalidWheel(format!(
                "unsupported Wheel-Version: {version}"
            )));
        }
        if minor > 0 {
            tracing::warn!(
                wheel_version = version,
                "wheel version is newer than supported version 1.0"
            );
        }
        let tags = wheel_txt
            .lines()
            .filter_map(|line| line.split_once(':'))
            .filter(|(key, _)| key.trim().eq_ignore_ascii_case("Tag"))
            .map(|(_, value)| value.trim().to_string())
            .collect();
        Ok(Self {
            dist_info: PathBuf::from(format!("{stem}.dist-info")),
            data: PathBuf::from(format!("{stem}.data")),
            tags,
        })
    }

    /// Reject conflicting destinations before writing files.
    fn plan<R: Read + Seek>(
        &self,
        archive: &mut ZipArchive<R>,
    ) -> Result<Vec<PlannedEntry>, ExtractError> {
        let mut destinations = Destinations::default();
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
                return Err(ExtractError::InvalidWheel(format!(
                    "duplicate archive path: {name}"
                )));
            }
            let target = self.target_path(&archive_path)?;
            if let Some(target) = &target {
                if target.starts_with("info") {
                    return Err(ExtractError::InvalidWheel(format!(
                        "wheel payload overlaps conda metadata: {}",
                        target.display()
                    )));
                }
                destinations.insert(target)?;
            }
            plan.push(PlannedEntry {
                index,
                archive_path,
                target,
            });
        }
        Ok(plan)
    }

    fn is_dist_info_file(&self, archive_path: &Path, file_name: &str) -> bool {
        archive_path == self.dist_info.join(file_name)
    }

    /// Maps wheel paths to conda paths, skipping replaced or discarded metadata.
    fn target_path(&self, archive_path: &Path) -> Result<Option<PathBuf>, ExtractError> {
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
                return Err(ExtractError::InvalidWheel(format!(
                    "unknown .data scheme '{other}' for {}",
                    archive_path.display()
                )));
            }
        };
        Ok(Some(target))
    }
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
    let mut file = std::fs::File::create_new(&full_path)?;
    let mut hashing = HashingReader::<_, Sha256>::new(&mut content);
    let size_in_bytes = io::copy(&mut hashing, &mut file)?;
    let (_, sha256) = hashing.finalize();

    #[cfg(unix)]
    if let Some(mode) = unix_mode {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&full_path, std::fs::Permissions::from_mode(mode & 0o777))?;
    }
    #[cfg(not(unix))]
    let _ = unix_mode;

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
        let entry_point = EntryPoint::from_str(spec).map_err(|err| {
            ExtractError::InvalidWheel(format!("invalid entry point '{line}': {err}"))
        })?;
        entry_points.push(entry_point);
    }
    Ok(entry_points)
}

/// Writes the generated conda metadata.
fn write_info(
    destination: &Path,
    record: &PackageRecord,
    layout: &WheelLayout,
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
        noarch: if record.noarch.is_none() {
            NoArchType::python()
        } else {
            record.noarch
        },
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
        dist_info: layout.dist_info.to_string_lossy().into_owned(),
        tags: layout.tags.clone(),
        sha256: record.sha256.map(hex::encode),
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
