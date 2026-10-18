//! Converts a Python wheel into the layout of an extracted conda package.
//!
//! A wheel served by a conda channel (a repodata v3 `whl` record) is a zip
//! file laid out per PEP 427. The conda installer only understands extracted
//! conda packages, so instead of teaching the linker about wheels, the wheel
//! is rewritten into that layout once, when it enters the package cache:
//!
//! * importable files go under `site-packages/`
//! * `{name}-{version}.data/purelib/*` and `.data/platlib/*` go under `site-packages/`
//! * `{name}-{version}.data/scripts/*` goes under `python-scripts/`
//! * `{name}-{version}.data/headers/*` goes under `include/`
//! * `{name}-{version}.data/data/*` goes to the package root (prefix relative)
//! * `<dist-info>/INSTALLER` is written as `conda` (and listed in `RECORD`) so
//!   Python package managers leave the files alone
//! * every file is checked against the hash and size its `RECORD` line claims,
//!   as a PEP 427 installer does; a disagreement rejects the wheel
//! * `info/index.json` is synthesized from the repodata record
//! * `info/paths.json` is synthesized from the files that were written
//! * `info/link.json` is synthesized from `entry_points.txt`
//! * `info/wheel.json` ([`WheelJson`]) marks the entry as a conversion and
//!   records [`CONVERSION_VERSION`], so a cache can refetch entries produced
//!   by an older or newer converter
//!
//! The linker then treats the result as an ordinary `noarch: python` package:
//! `site-packages/` and `python-scripts/` are remapped for the target Python,
//! entry points are generated, `.pyc` files are compiled, and `conda-meta` is
//! written as for any other package.
//!
//! Scripts under `.data/scripts` keep whatever shebang the wheel shipped,
//! typically the `#!python` placeholder that PEP 427 installers rewrite. The
//! conda linker does not rewrite it. Console entry points, which cover the
//! vast majority of command line tools on `PyPI`, are unaffected.

use std::{
    collections::HashMap,
    io::{self, Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

use rattler_conda_types::{
    NoArchType, PackageRecord,
    package::{
        EntryPoint, IndexJson, LinkJson, NoArchLinks, PackageFile, PathType, PathsEntry, PathsJson,
        PythonEntryPoints,
    },
};
use rattler_digest::{HashingReader, Sha256, Sha256Hash};
use serde::{Deserialize, Serialize};
use tempfile::SpooledTempFile;
use zip::ZipArchive;

use crate::{ExtractError, ExtractResult};

/// Directory, relative to the package root, where importable files go. The
/// linker remaps it to the `site-packages` directory of the target Python.
const SITE_PACKAGES: &str = "site-packages";

/// Directory, relative to the package root, where scripts go. The linker
/// remaps it to the `bin` (or `Scripts`) directory of the prefix.
const PYTHON_SCRIPTS: &str = "python-scripts";

/// The value written to `<dist-info>/INSTALLER`.
const INSTALLER: &str = "conda\n";

/// Wheels can be spooled to memory up to this size before spilling to disk.
const SPOOL_LIMIT: usize = 5 * 1024 * 1024;

/// The version of the wheel to conda layout conversion implemented by this
/// crate. Bump it whenever the layout written for the same wheel changes, so
/// that caches refetch entries converted by other versions.
pub const CONVERSION_VERSION: u64 = 1;

/// The `info/wheel.json` file written into every converted wheel.
///
/// A converted entry is derived data: the wheel's hash and `paths.json` both
/// stay valid when the converter changes, so this file is what lets a cache
/// tell an outdated conversion from a current one. It also marks the entry's
/// origin for anyone inspecting the cache, since nothing else in the layout
/// does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WheelJson {
    /// The [`CONVERSION_VERSION`] of the converter that wrote this entry.
    pub conversion_version: u64,

    /// The wheel's `.dist-info` directory name, for example
    /// `cowsay-6.1.dist-info`.
    pub dist_info: String,

    /// The compatibility tags from the wheel's `WHEEL` file, for example
    /// `py3-none-any`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// Hex sha256 of the wheel archive, as the repodata record stated it.
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

/// Extracts a wheel from `reader` into `destination` as an extracted conda
/// package, synthesizing the `info/` metadata from `record`.
///
/// Returns the hashes and size of the wheel archive itself, so callers can
/// verify them against the repodata record exactly as for a conda archive.
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
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;

    // Zip archives need random access, so buffer the stream first.
    let mut temp_file = SpooledTempFile::new(SPOOL_LIMIT);
    io::copy(&mut reader, &mut temp_file)?;
    temp_file.seek(SeekFrom::Start(0))?;
    let mut archive = ZipArchive::new(temp_file)?;

    let layout = WheelLayout::detect(&mut archive)?;
    let manifest = RecordManifest::read(&mut archive, &layout)?;
    let mut paths = Vec::with_capacity(archive.len() + 1);
    let mut entry_points = Vec::new();

    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        if file.is_dir() {
            continue;
        }
        let Some(archive_path) = file.enclosed_name() else {
            return Err(ExtractError::InvalidArchivePath(PathBuf::from(
                file.name()?.into_owned(),
            )));
        };
        let Some(target) = layout.target_path(&archive_path)? else {
            // Installer-owned files are regenerated below.
            continue;
        };

        let entry = if layout.is_dist_info_file(&archive_path, "RECORD") {
            // Installers add themselves to RECORD; mirror that for INSTALLER.
            let mut record_txt = String::new();
            file.read_to_string(&mut record_txt)?;
            let installer_line = format!("{}/INSTALLER,,\n", layout.dist_info.display());
            if !record_txt.contains(&installer_line) {
                if !record_txt.is_empty() && !record_txt.ends_with('\n') {
                    record_txt.push('\n');
                }
                record_txt.push_str(&installer_line);
            }
            write_entry(destination, target, record_txt.as_bytes(), None)?
        } else if layout.is_dist_info_file(&archive_path, "entry_points.txt") {
            let mut entry_points_txt = String::new();
            file.read_to_string(&mut entry_points_txt)?;
            entry_points = parse_entry_points(&entry_points_txt)?;
            write_entry(destination, target, entry_points_txt.as_bytes(), None)?
        } else {
            let mode = file.unix_mode();
            write_entry(destination, target, &mut file, mode)?
        };
        manifest.verify(&archive_path, &entry)?;
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
    write_info(destination, record, &layout, paths, entry_points)
}

/// What the wheel's `RECORD` claims about each file: the sha256 and size a
/// PEP 427 installer verifies before trusting the file.
///
/// `RECORD` is a second opinion, not the source of `paths.json`: the archive
/// drives the walk and the hashes are computed from the bytes written. A file
/// that `RECORD` does not mention is allowed, as some wheels in the wild
/// have them, and `RECORD` itself and signature files carry no hash by spec.
#[derive(Default)]
struct RecordManifest {
    entries: HashMap<String, RecordEntry>,
}

struct RecordEntry {
    sha256: Option<Sha256Hash>,
    size: Option<u64>,
}

impl RecordManifest {
    fn read<R: Read + Seek>(
        archive: &mut ZipArchive<R>,
        layout: &WheelLayout,
    ) -> Result<Self, ExtractError> {
        let record_name = format!("{}/RECORD", layout.dist_info.display());
        let Some(index) = archive.index_for_name(&record_name) else {
            return Ok(Self::default());
        };
        let mut record_txt = String::new();
        archive.by_index(index)?.read_to_string(&mut record_txt)?;
        Self::parse(&record_txt)
    }

    fn parse(record_txt: &str) -> Result<Self, ExtractError> {
        let mut entries = HashMap::new();
        for line in record_txt.lines() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() {
                continue;
            }
            // `path,hash,size`, split from the right because a quoted path
            // may itself contain commas.
            let mut fields = line.rsplitn(3, ',');
            let (Some(size), Some(hash), Some(path)) =
                (fields.next(), fields.next(), fields.next())
            else {
                return Err(ExtractError::InvalidWheel(format!(
                    "malformed RECORD line: {line}"
                )));
            };
            let path = path.trim_matches('"').to_string();
            let sha256 = match hash.split_once('=') {
                Some(("sha256", digest)) => {
                    let bytes = URL_SAFE_NO_PAD.decode(digest).map_err(|err| {
                        ExtractError::InvalidWheel(format!(
                            "RECORD has an invalid sha256 for {path}: {err}"
                        ))
                    })?;
                    let Ok(digest) = Sha256Hash::try_from(bytes.as_slice()) else {
                        return Err(ExtractError::InvalidWheel(format!(
                            "RECORD has a sha256 of {} bytes for {path}",
                            bytes.len()
                        )));
                    };
                    Some(digest)
                }
                // Other algorithms are permitted by the spec; nothing to compare.
                _ => None,
            };
            let size = if size.is_empty() {
                None
            } else {
                Some(size.parse::<u64>().map_err(|err| {
                    ExtractError::InvalidWheel(format!(
                        "RECORD has an invalid size for {path}: {err}"
                    ))
                })?)
            };
            entries.insert(path, RecordEntry { sha256, size });
        }
        Ok(Self { entries })
    }

    /// Checks a written file against its `RECORD` line, if it has one.
    fn verify(&self, archive_path: &Path, written: &PathsEntry) -> Result<(), ExtractError> {
        let key = archive_path.to_string_lossy();
        let Some(claimed) = self.entries.get(key.as_ref()) else {
            return Ok(());
        };
        if let (Some(claimed), Some(actual)) = (claimed.sha256, written.sha256)
            && claimed != actual
        {
            return Err(ExtractError::InvalidWheel(format!(
                "{key}: RECORD claims sha256 {} but the archive holds {}",
                hex::encode(claimed),
                hex::encode(actual)
            )));
        }
        if let (Some(claimed), Some(actual)) = (claimed.size, written.size_in_bytes)
            && claimed != actual
        {
            return Err(ExtractError::InvalidWheel(format!(
                "{key}: RECORD claims {claimed} bytes but the archive holds {actual}"
            )));
        }
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

    fn is_dist_info_file(&self, archive_path: &Path, file_name: &str) -> bool {
        archive_path == self.dist_info.join(file_name)
    }

    /// Maps a path inside the wheel to its path inside the conda package, or
    /// `None` when the file must not be copied.
    fn target_path(&self, archive_path: &Path) -> Result<Option<PathBuf>, ExtractError> {
        if self.is_dist_info_file(archive_path, "INSTALLER") {
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

/// Writes `content` to `destination/relative_path`, hashing it on the way, and
/// returns the matching `paths.json` entry.
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
    let mut file = std::fs::File::create(&full_path)?;
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

/// Parses the `[console_scripts]` and `[gui_scripts]` sections of an
/// `entry_points.txt` file into conda entry points.
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

/// Writes `info/index.json`, `info/paths.json` and `info/link.json`.
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
        // The layout written above only makes sense for a noarch python
        // package, which is also what the repodata spec requires for wheels.
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
    let json = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    std::fs::write(full_path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{path::Path, str::FromStr};

    use rattler_conda_types::{
        PackageName, PackageRecord, Version,
        package::{BuildString, IndexJson, LinkJson, NoArchLinks, PackageFile, PathsJson},
    };
    use rattler_digest::{Sha256, compute_bytes_digest, compute_file_digest};

    use super::*;

    fn fixture_wheel() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-data/wheels/rattler_fixture-1.0.0-py3-none-any.whl")
    }

    fn fixture_record() -> PackageRecord {
        PackageRecord {
            subdir: "noarch".to_string(),
            noarch: NoArchType::python(),
            depends: vec![
                "python >=3.8".to_string(),
                "typing_extensions >=4".to_string(),
            ],
            license: Some("MIT".to_string()),
            ..PackageRecord::new(
                PackageName::new_unchecked("rattler-fixture"),
                Version::from_str("1.0.0").unwrap(),
                "py3_none_any_0".parse::<BuildString>().unwrap(),
            )
        }
    }

    #[test]
    fn test_extract_wheel_writes_conda_layout() {
        let destination = tempfile::tempdir().unwrap();
        let wheel = std::fs::File::open(fixture_wheel()).unwrap();
        let result = extract_wheel(wheel, destination.path(), &fixture_record()).unwrap();

        // The result describes the wheel archive, so the cache can verify it
        // against the repodata record.
        let expected = compute_file_digest::<Sha256>(fixture_wheel()).unwrap();
        assert_eq!(result.sha256, expected);
        assert_eq!(
            result.total_size,
            std::fs::metadata(fixture_wheel()).unwrap().len()
        );

        let dest = destination.path();
        let dist_info = "site-packages/rattler_fixture-1.0.0.dist-info";
        for expected in [
            "site-packages/rattler_fixture/__init__.py",
            "site-packages/rattler_fixture/__main__.py",
            "python-scripts/rattler-fixture-script",
            "share/rattler_fixture/cow.txt",
            &format!("{dist_info}/METADATA"),
            &format!("{dist_info}/WHEEL"),
            &format!("{dist_info}/RECORD"),
            &format!("{dist_info}/entry_points.txt"),
            &format!("{dist_info}/INSTALLER"),
            "info/index.json",
            "info/paths.json",
            "info/link.json",
            "info/wheel.json",
        ] {
            assert!(dest.join(expected).is_file(), "missing {expected}");
        }
        assert!(!dest.join("rattler_fixture-1.0.0.data").exists());
        assert!(
            !dest
                .join("site-packages/rattler_fixture-1.0.0.data")
                .exists()
        );

        assert_eq!(
            std::fs::read_to_string(dest.join(dist_info).join("INSTALLER")).unwrap(),
            "conda\n"
        );
        let record_txt = std::fs::read_to_string(dest.join(dist_info).join("RECORD")).unwrap();
        assert!(record_txt.contains("rattler_fixture-1.0.0.dist-info/INSTALLER,,\n"));
        assert!(record_txt.contains("rattler_fixture/__init__.py,sha256="));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dest.join("python-scripts/rattler-fixture-script"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "scripts keep their executable bit");
        }
    }

    #[test]
    fn test_extract_wheel_synthesizes_info_files() {
        let destination = tempfile::tempdir().unwrap();
        let wheel = std::fs::File::open(fixture_wheel()).unwrap();
        extract_wheel(wheel, destination.path(), &fixture_record()).unwrap();
        let dest = destination.path();

        let index_json = IndexJson::from_package_directory(dest).unwrap();
        assert_eq!(index_json.name.as_normalized(), "rattler-fixture");
        assert_eq!(index_json.version.to_string(), "1.0.0");
        assert_eq!(index_json.build.as_str(), "py3_none_any_0");
        assert!(index_json.noarch.is_python());
        assert_eq!(index_json.subdir.as_deref(), Some("noarch"));
        assert_eq!(
            index_json.depends,
            ["python >=3.8", "typing_extensions >=4"]
        );
        assert_eq!(index_json.license.as_deref(), Some("MIT"));

        let link_json = LinkJson::from_package_directory(dest).unwrap();
        let NoArchLinks::Python(links) = link_json.noarch else {
            panic!("a wheel must become a noarch python package");
        };
        let commands: Vec<_> = links
            .entry_points
            .iter()
            .map(|ep| {
                (
                    ep.command.as_str(),
                    ep.module.as_str(),
                    ep.function.as_str(),
                )
            })
            .collect();
        assert_eq!(
            commands,
            [
                ("rattler-fixture", "rattler_fixture", "main"),
                ("rattler-fixture-gui", "rattler_fixture", "main"),
            ]
        );

        let wheel_json = WheelJson::from_package_directory(dest).unwrap();
        assert_eq!(
            wheel_json,
            WheelJson {
                conversion_version: CONVERSION_VERSION,
                dist_info: "rattler_fixture-1.0.0.dist-info".to_string(),
                tags: vec!["py3-none-any".to_string()],
                sha256: None,
            }
        );

        let paths_json = PathsJson::from_package_directory(dest).unwrap();
        assert!(
            paths_json
                .paths
                .windows(2)
                .all(|pair| pair[0].relative_path < pair[1].relative_path),
            "paths.json entries are sorted"
        );
        let listed: Vec<_> = paths_json
            .paths
            .iter()
            .map(|p| p.relative_path.to_string_lossy().into_owned())
            .collect();
        assert!(
            listed.contains(&"site-packages/rattler_fixture-1.0.0.dist-info/INSTALLER".to_string())
        );
        assert!(listed.contains(&"python-scripts/rattler-fixture-script".to_string()));
        assert!(listed.contains(&"share/rattler_fixture/cow.txt".to_string()));
        assert!(!listed.iter().any(|p| p.starts_with("info/")));

        // Every entry carries the hash and size of the file that was written.
        for entry in &paths_json.paths {
            let content = std::fs::read(dest.join(&entry.relative_path)).unwrap();
            assert_eq!(
                entry.size_in_bytes,
                Some(content.len() as u64),
                "{}",
                entry.relative_path.display()
            );
            assert_eq!(
                entry.sha256,
                Some(compute_bytes_digest::<Sha256>(&content)),
                "{}",
                entry.relative_path.display()
            );
        }
    }

    #[test]
    fn test_parse_entry_points_handles_extras_and_other_sections() {
        let entry_points = parse_entry_points(
            "[other.plugins]\nx = a:b\n\n[console_scripts]\n# a comment\nfoo = pkg.cli:main [extra1,extra2]\n\n[gui_scripts]\nfoo-gui = pkg.gui:run\n",
        )
        .unwrap();
        let commands: Vec<_> = entry_points.iter().map(ToString::to_string).collect();
        assert_eq!(commands, ["foo = pkg.cli:main", "foo-gui = pkg.gui:run"]);
    }

    /// Builds a wheel in memory from `(path, content)` pairs, with the given
    /// RECORD text.
    fn wheel_with_record(files: &[(&str, &[u8])], record: &str) -> Vec<u8> {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            for (path, content) in files {
                zip.start_file(*path, options).unwrap();
                std::io::Write::write_all(&mut zip, content).unwrap();
            }
            zip.start_file("pkg-1.0.dist-info/WHEEL", options).unwrap();
            std::io::Write::write_all(&mut zip, b"Wheel-Version: 1.0\n").unwrap();
            zip.start_file("pkg-1.0.dist-info/RECORD", options).unwrap();
            std::io::Write::write_all(&mut zip, record.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        buffer.into_inner()
    }

    #[test]
    fn test_extract_wheel_rejects_record_hash_mismatch() {
        let content = b"print('hi')\n";
        let wrong = URL_SAFE_NO_PAD.encode(compute_bytes_digest::<Sha256>(b"something else"));
        let wheel = wheel_with_record(
            &[("pkg/__init__.py", content)],
            &format!(
                "pkg/__init__.py,sha256={wrong},{}\npkg-1.0.dist-info/RECORD,,\n",
                content.len()
            ),
        );
        let destination = tempfile::tempdir().unwrap();
        let err = extract_wheel(
            std::io::Cursor::new(wheel),
            destination.path(),
            &fixture_record(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, ExtractError::InvalidWheel(msg) if msg.contains("pkg/__init__.py") && msg.contains("sha256")),
            "{err}"
        );
    }

    #[test]
    fn test_extract_wheel_rejects_record_size_mismatch() {
        let content = b"print('hi')\n";
        let right = URL_SAFE_NO_PAD.encode(compute_bytes_digest::<Sha256>(content));
        let wheel = wheel_with_record(
            &[("pkg/__init__.py", content)],
            &format!("pkg/__init__.py,sha256={right},999\npkg-1.0.dist-info/RECORD,,\n"),
        );
        let destination = tempfile::tempdir().unwrap();
        let err = extract_wheel(
            std::io::Cursor::new(wheel),
            destination.path(),
            &fixture_record(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, ExtractError::InvalidWheel(msg) if msg.contains("999 bytes")),
            "{err}"
        );
    }

    #[test]
    fn test_record_manifest_tolerates_unlisted_files_and_other_algorithms() {
        let content = b"data\n";
        let wheel = wheel_with_record(
            &[("pkg/listed.py", content), ("pkg/unlisted.py", content)],
            "pkg/listed.py,md5=ignored,5\npkg-1.0.dist-info/RECORD,,\n",
        );
        let destination = tempfile::tempdir().unwrap();
        extract_wheel(
            std::io::Cursor::new(wheel),
            destination.path(),
            &fixture_record(),
        )
        .unwrap();
        assert!(
            destination
                .path()
                .join("site-packages/pkg/unlisted.py")
                .is_file()
        );
    }

    #[test]
    fn test_record_manifest_parses_quoted_paths_and_empty_fields() {
        let manifest = RecordManifest::parse(
            "\"dir/with,comma.py\",sha256=47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU,0\r\npkg-1.0.dist-info/RECORD,,\n",
        )
        .unwrap();
        let entry = &manifest.entries["dir/with,comma.py"];
        assert_eq!(entry.size, Some(0));
        assert_eq!(entry.sha256, Some(compute_bytes_digest::<Sha256>(b"")));
        let record = &manifest.entries["pkg-1.0.dist-info/RECORD"];
        assert!(record.sha256.is_none() && record.size.is_none());
    }

    #[test]
    fn test_extract_wheel_rejects_archive_without_dist_info() {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            zip.start_file("pkg/__init__.py", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.finish().unwrap();
        }
        let destination = tempfile::tempdir().unwrap();
        let err = extract_wheel(
            std::io::Cursor::new(buffer.into_inner()),
            destination.path(),
            &fixture_record(),
        )
        .unwrap_err();
        assert!(matches!(err, ExtractError::InvalidWheel(_)), "{err}");
    }
}
