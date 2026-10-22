//! Installing packages straight from their archives, without extracting them to a package cache
//! or writing a prefix to disk.
//!
//! The regular installer ([`crate::install::Installer`]) extracts every package into the package
//! cache and then links the files into a prefix on the local filesystem. That only works if the
//! local filesystem can represent every file of the package, which is not the case when the
//! packages are meant for another platform: a Linux package installed on Windows loses its
//! symlinks and executable bits, and files whose names only differ in case collide.
//!
//! This module instead reads the files of a package directly from its `.conda` or `.tar.bz2`
//! archive and hands them, with prefix placeholders already replaced, to a [`PrefixSink`]. The sink
//! decides what happens with them, for example writing them into a tar archive or an OCI layer.
//!
//! Linking happens in two passes:
//!
//! 1. [`ArchivePackage::from_path`] reads only the metadata (`info/`) of a package.
//! 2. [`LinkPlan::new`] takes the metadata of all packages of an environment and decides where
//!    every file ends up, including the files that are installed by multiple packages
//!    ("clobbering") and the remapping of `noarch: python` packages.
//! 3. [`PlannedPackage::link`] streams the contents of a single package into a [`PrefixSink`] and
//!    returns the [`PrefixRecord`] that describes it. Packages are independent of each other at
//!    this point, so they can be linked in parallel and into separate sinks.
//!
//! # Example
//!
//! ```rust,no_run
//! # use std::{io::Read, path::Path};
//! use rattler::install::from_archive::{
//!     ArchiveLinkOptions, ArchivePackage, LinkPlan, PrefixSink,
//! };
//! use rattler_conda_types::{RepoDataRecord, Subdir};
//!
//! struct PrintSink;
//!
//! impl PrefixSink for PrintSink {
//!     fn add_file(
//!         &mut self,
//!         path: &Path,
//!         mode: u32,
//!         size: u64,
//!         _contents: &mut dyn Read,
//!     ) -> std::io::Result<()> {
//!         println!("{} ({mode:o}, {size} bytes)", path.display());
//!         Ok(())
//!     }
//!
//!     fn add_symlink(&mut self, path: &Path, target: &Path) -> std::io::Result<()> {
//!         println!("{} -> {}", path.display(), target.display());
//!         Ok(())
//!     }
//!
//!     fn add_directory(&mut self, path: &Path, _mode: u32) -> std::io::Result<()> {
//!         println!("{}/", path.display());
//!         Ok(())
//!     }
//! }
//!
//! # fn example(records: Vec<(RepoDataRecord, std::path::PathBuf)>) -> Result<(), Box<dyn std::error::Error>> {
//! let packages = records
//!     .into_iter()
//!     .map(|(record, archive)| Ok((record, ArchivePackage::from_path(archive)?)))
//!     .collect::<Result<Vec<_>, rattler::install::from_archive::FromArchiveError>>()?;
//! let plan = LinkPlan::new(packages, ArchiveLinkOptions::new("/opt/env", Subdir::Linux64))?;
//! for package in plan.packages() {
//!     let prefix_record = package.link(&mut PrintSink)?;
//!     println!("conda-meta/{}", prefix_record.file_name());
//! }
//! # Ok(())
//! # }
//! ```

mod clobber;

use std::{
    collections::HashMap,
    fs::File,
    io::{self, BufReader, Read},
    path::{Component, Path, PathBuf},
};

use rattler_conda_types::{
    PackageName, PackageRecord, PrefixRecord, RepoDataRecord, Subdir,
    package::{
        CondaArchiveType, FileMode, Files, HasPrefix, IndexJson, LinkJson, NoArchLinks, NoLink,
        NoSoftlink, PackageFile, PathType, PathsEntry, PathsJson,
    },
    prefix_record,
};
use rattler_digest::{HashingReader, Sha256, Sha256Hash};
use rattler_package_streaming::{
    ExtractError,
    read::stream_tar_bz2,
    seek::{stream_conda_content, stream_conda_info},
};

pub use self::clobber::{ClobberResolution, resolve_clobbers};
use super::{
    AppleCodeSignBehavior, ClobberMode, ExternalSymlinkPolicy, PythonInfo,
    clobber_registry::{CLOBBERS_DIR_NAME, ClobberedPath},
    entry_point::{EntryPointFile, python_entry_point_files},
    link::{FileType, LinkFileError, replace_prefix_placeholder},
    python::PythonInfoError,
};

/// An error that can occur while installing packages from their archives.
#[derive(Debug, thiserror::Error)]
pub enum FromArchiveError {
    /// The file is not a `.conda` or `.tar.bz2` archive.
    #[error("'{0}' is not a conda package archive")]
    UnsupportedArchiveType(PathBuf),

    /// The archive could not be opened.
    #[error("failed to open '{0}'")]
    FailedToOpenArchive(PathBuf, #[source] io::Error),

    /// The archive could not be read.
    #[error("failed to read '{0}'")]
    FailedToReadArchive(PathBuf, #[source] Box<ExtractError>),

    /// A metadata file in the `info/` directory of the archive is missing or invalid.
    #[error("failed to read '{1}' from '{0}'")]
    InvalidMetadata(PathBuf, PathBuf, #[source] io::Error),

    /// A path of a package is absolute or leaves the prefix.
    #[error("package '{0}' contains the path '{1}' which is outside of the prefix")]
    PathOutsideOfPrefix(String, PathBuf),

    /// A noarch python package is installed without python information.
    #[error(
        "cannot install noarch python package '{0}' because there is no python version specified"
    )]
    MissingPythonInfo(String),

    /// The python information could not be determined from the python package.
    #[error("failed to determine the python version of the environment")]
    InvalidPythonInfo(#[source] PythonInfoError),

    /// Failed to create a python entry point.
    #[error("failed to create the python entry points of '{0}'")]
    FailedToCreateEntryPoint(String, #[source] io::Error),

    /// Multiple packages install the same file and [`ClobberMode::Error`] was requested.
    #[error("multiple packages install the same files: {}", format_clobbered(.0))]
    ClobberingDetected(HashMap<PathBuf, ClobberedPath>),

    /// A file listed in `paths.json` is missing from the archive.
    #[error("'{1}' is listed in the paths.json of '{0}' but is missing from the archive")]
    MissingFile(String, PathBuf),

    /// A file in the archive has a type that cannot be installed.
    #[error("'{1}' in '{0}' has an unsupported type")]
    UnsupportedEntryType(String, PathBuf),

    /// Replacing the prefix placeholder of a file failed.
    #[error("failed to link '{0}'")]
    FailedToLink(PathBuf, #[source] LinkFileError),

    /// A symlink points outside of the prefix and [`ExternalSymlinkPolicy::Deny`] was requested.
    #[error("the symlink '{0}' points outside of the prefix")]
    SymlinkTargetEscapesPrefix(PathBuf),

    /// A macOS binary was modified by prefix replacement and would need to be signed again,
    /// which is not supported when linking from an archive.
    #[error(
        "'{0}' needs to be code signed after prefix replacement, which is not supported when \
         linking from an archive"
    )]
    CodeSigningRequired(PathBuf),

    /// The sink returned an error.
    #[error("failed to write '{0}'")]
    SinkError(PathBuf, #[source] io::Error),
}

fn format_clobbered(paths: &HashMap<PathBuf, ClobberedPath>) -> String {
    let mut paths = paths
        .iter()
        .map(|(path, clobbered)| {
            format!(
                "'{}' ({}, {})",
                path.display(),
                clobbered.package.as_normalized(),
                clobbered
                    .other_packages
                    .iter()
                    .map(PackageName::as_normalized)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths.join(", ")
}

/// Receives the files of a package from [`PlannedPackage::link`].
///
/// All paths are relative to the prefix and only contain normal components. Parent directories
/// are not reported separately, only directories that a package explicitly creates (empty
/// directories) are passed to [`PrefixSink::add_directory`].
pub trait PrefixSink {
    /// Adds a regular file with the unix permission bits `mode`. `contents` yields exactly `size`
    /// bytes. Bytes that are not read by the sink are skipped.
    fn add_file(
        &mut self,
        path: &Path,
        mode: u32,
        size: u64,
        contents: &mut dyn Read,
    ) -> io::Result<()>;

    /// Adds a symbolic link that points to `target`.
    fn add_symlink(&mut self, path: &Path, target: &Path) -> io::Result<()>;

    /// Adds an (empty) directory with the unix permission bits `mode`.
    fn add_directory(&mut self, path: &Path, mode: u32) -> io::Result<()>;
}

impl<S: PrefixSink + ?Sized> PrefixSink for &mut S {
    fn add_file(
        &mut self,
        path: &Path,
        mode: u32,
        size: u64,
        contents: &mut dyn Read,
    ) -> io::Result<()> {
        (**self).add_file(path, mode, size, contents)
    }

    fn add_symlink(&mut self, path: &Path, target: &Path) -> io::Result<()> {
        (**self).add_symlink(path, target)
    }

    fn add_directory(&mut self, path: &Path, mode: u32) -> io::Result<()> {
        (**self).add_directory(path, mode)
    }
}

/// The metadata of a package archive on disk, read by [`ArchivePackage::from_path`].
#[derive(Debug, Clone)]
pub struct ArchivePackage {
    path: PathBuf,
    archive_type: CondaArchiveType,

    /// The `info/index.json` of the package.
    pub index_json: IndexJson,

    /// The `info/paths.json` of the package, or the equivalent information from the deprecated
    /// `info/files`, `info/has_prefix`, `info/no_link` and `info/no_softlink` files.
    pub paths_json: PathsJson,

    /// The `info/link.json` of the package, if it has one.
    pub link_json: Option<LinkJson>,
}

impl ArchivePackage {
    /// Reads the metadata of the `.conda` or `.tar.bz2` archive at `path`.
    ///
    /// For `.conda` archives only the (small) `info` part of the archive is read. `.tar.bz2`
    /// archives have to be decompressed completely because the metadata can be stored anywhere in
    /// the archive.
    pub fn from_path(path: impl Into<PathBuf>) -> Result<Self, FromArchiveError> {
        let path = path.into();
        let archive_type = CondaArchiveType::try_from(&path)
            .ok_or_else(|| FromArchiveError::UnsupportedArchiveType(path.clone()))?;

        let info = read_info_files(&path, archive_type)?;
        let index_json = info
            .parse::<IndexJson>(&path)?
            .ok_or_else(|| missing_metadata(&path, IndexJson::package_path()))?;
        let link_json = info.parse::<LinkJson>(&path)?;

        let paths_json = if let Some(paths_json) = info.parse::<PathsJson>(&path)? {
            paths_json
        } else {
            // Old packages do not have a `paths.json`, the information is spread over other
            // files instead.
            let files = info
                .parse::<Files>(&path)?
                .ok_or_else(|| missing_metadata(&path, Files::package_path()))?;
            let has_prefix = info.parse::<HasPrefix>(&path)?;
            let no_link = info.parse::<NoLink>(&path)?;
            let no_softlink = info.parse::<NoSoftlink>(&path)?;
            let entry_types = match info.entry_types {
                Some(entry_types) => entry_types,
                None => read_content_entry_types(&path, archive_type)?,
            };
            PathsJson::from_deprecated(files, has_prefix, no_link, no_softlink, |path| {
                Ok::<_, std::convert::Infallible>(match entry_types.get(path) {
                    Some(tar::EntryType::Symlink) => PathType::SoftLink,
                    Some(tar::EntryType::Directory) => PathType::Directory,
                    _ => PathType::HardLink,
                })
            })
            .unwrap_or_else(|never| match never {})
        };

        Ok(Self {
            path,
            archive_type,
            index_json,
            paths_json,
            link_json,
        })
    }

    /// The location of the archive.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The type of the archive.
    pub fn archive_type(&self) -> CondaArchiveType {
        self.archive_type
    }

    /// Calls `f` for every entry in the content part of the archive (the part outside of `info/`).
    fn for_each_content_entry(
        &self,
        f: impl FnMut(&Path, &mut tar::Entry<'_, &mut dyn Read>) -> Result<(), FromArchiveError>,
    ) -> Result<(), FromArchiveError> {
        for_each_content_entry(&self.path, self.archive_type, f)
    }
}

/// The metadata files read from the `info/` directory of an archive.
struct InfoFiles {
    files: HashMap<PathBuf, Vec<u8>>,

    /// The types of all content entries, only available for `.tar.bz2` archives because they are
    /// read completely anyway.
    entry_types: Option<HashMap<PathBuf, tar::EntryType>>,
}

impl InfoFiles {
    /// Parses the metadata file `P`, returns `None` if the archive does not contain it.
    fn parse<P: PackageFile>(&self, archive: &Path) -> Result<Option<P>, FromArchiveError> {
        self.files
            .get(P::package_path())
            .map(|bytes| {
                P::from_slice(bytes).map_err(|e| {
                    FromArchiveError::InvalidMetadata(archive.into(), P::package_path().into(), e)
                })
            })
            .transpose()
    }
}

fn missing_metadata(archive: &Path, name: &Path) -> FromArchiveError {
    FromArchiveError::InvalidMetadata(archive.into(), name.into(), io::ErrorKind::NotFound.into())
}

/// The metadata files that [`ArchivePackage::from_path`] needs.
fn is_wanted_info_file(path: &Path) -> bool {
    [
        IndexJson::package_path(),
        PathsJson::package_path(),
        LinkJson::package_path(),
        Files::package_path(),
        HasPrefix::package_path(),
        NoLink::package_path(),
        NoSoftlink::package_path(),
    ]
    .contains(&path)
}

fn read_info_files(
    path: &Path,
    archive_type: CondaArchiveType,
) -> Result<InfoFiles, FromArchiveError> {
    let read_error =
        |e: io::Error| FromArchiveError::FailedToReadArchive(path.into(), Box::new(e.into()));
    let file =
        File::open(path).map_err(|e| FromArchiveError::FailedToOpenArchive(path.into(), e))?;

    let mut files = HashMap::new();
    let mut collect = |entry_path: PathBuf, entry: &mut dyn Read| -> io::Result<()> {
        if is_wanted_info_file(&entry_path) {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            files.insert(entry_path, bytes);
        }
        Ok(())
    };

    match archive_type {
        CondaArchiveType::Conda => {
            let mut archive = stream_conda_info(BufReader::new(file))
                .map_err(|e| FromArchiveError::FailedToReadArchive(path.into(), Box::new(e)))?;
            for entry in archive.entries().map_err(read_error)? {
                let mut entry = entry.map_err(read_error)?;
                let entry_path = normalize_entry_path(&entry.path().map_err(read_error)?);
                collect(entry_path, &mut entry).map_err(read_error)?;
            }
            Ok(InfoFiles {
                files,
                entry_types: None,
            })
        }
        CondaArchiveType::TarBz2 => {
            let mut entry_types = HashMap::new();
            let mut archive = stream_tar_bz2(BufReader::new(file));
            for entry in archive.entries().map_err(read_error)? {
                let mut entry = entry.map_err(read_error)?;
                let entry_path = normalize_entry_path(&entry.path().map_err(read_error)?);
                if entry_path.starts_with("info") {
                    collect(entry_path, &mut entry).map_err(read_error)?;
                } else {
                    entry_types.insert(entry_path, entry.header().entry_type());
                }
            }
            Ok(InfoFiles {
                files,
                entry_types: Some(entry_types),
            })
        }
    }
}

/// Reads the types of all content entries without reading their contents.
fn read_content_entry_types(
    path: &Path,
    archive_type: CondaArchiveType,
) -> Result<HashMap<PathBuf, tar::EntryType>, FromArchiveError> {
    let mut entry_types = HashMap::new();
    for_each_content_entry(path, archive_type, |entry_path, entry| {
        entry_types.insert(entry_path.to_path_buf(), entry.header().entry_type());
        Ok(())
    })?;
    Ok(entry_types)
}

fn for_each_content_entry(
    path: &Path,
    archive_type: CondaArchiveType,
    mut f: impl FnMut(&Path, &mut tar::Entry<'_, &mut dyn Read>) -> Result<(), FromArchiveError>,
) -> Result<(), FromArchiveError> {
    let read_error =
        |e: io::Error| FromArchiveError::FailedToReadArchive(path.into(), Box::new(e.into()));
    let file =
        File::open(path).map_err(|e| FromArchiveError::FailedToOpenArchive(path.into(), e))?;

    let mut decoder: Box<dyn Read> = match archive_type {
        CondaArchiveType::Conda => Box::new(
            stream_conda_content(BufReader::new(file))
                .map_err(|e| FromArchiveError::FailedToReadArchive(path.into(), Box::new(e)))?
                .into_inner(),
        ),
        CondaArchiveType::TarBz2 => Box::new(stream_tar_bz2(BufReader::new(file)).into_inner()),
    };
    let mut archive = tar::Archive::new(&mut *decoder as &mut dyn Read);
    for entry in archive.entries().map_err(read_error)? {
        let mut entry = entry.map_err(read_error)?;
        let entry_path = normalize_entry_path(&entry.path().map_err(read_error)?);
        if archive_type == CondaArchiveType::TarBz2 && entry_path.starts_with("info") {
            continue;
        }
        f(&entry_path, &mut entry)?;
    }
    Ok(())
}

/// Removes `.` components from a path in a tar archive (`./bin/foo` -> `bin/foo`).
fn normalize_entry_path(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect()
}

/// Returns true if `path` is relative and does not leave the directory it is relative to.
fn is_contained_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)))
}

/// Returns true if a symlink at `link_path` (relative to the prefix) pointing to `target` resolves
/// to a location outside of the prefix.
fn symlink_escapes_prefix(link_path: &Path, target: &Path) -> bool {
    if target.has_root() {
        return true;
    }
    let mut depth = 0usize;
    let parent = link_path.parent().unwrap_or(Path::new(""));
    for component in parent.components().chain(target.components()) {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => match depth.checked_sub(1) {
                Some(new_depth) => depth = new_depth,
                None => return true,
            },
            Component::RootDir | Component::Prefix(_) => return true,
        }
    }
    false
}

/// Options for [`LinkPlan::new`].
#[derive(Debug, Clone)]
pub struct ArchiveLinkOptions {
    /// The path of the prefix that the packages are installed into. Prefix placeholders in files
    /// are replaced with this path.
    pub target_prefix: String,

    /// The platform the packages are installed for.
    pub platform: Subdir,

    /// Information about the python installation of the environment, used to install `noarch:
    /// python` packages. If this is `None` it is derived from the `python` package of the
    /// environment (if there is one).
    pub python_info: Option<PythonInfo>,

    /// What to do when multiple packages install the same file.
    pub clobber_mode: ClobberMode,

    /// What to do when a macOS binary needs to be signed again after its prefix placeholder was
    /// replaced. Linking from an archive cannot sign binaries, so [`AppleCodeSignBehavior::Fail`]
    /// (the default) fails in that case.
    pub apple_codesign_behavior: AppleCodeSignBehavior,

    /// What to do with symlinks that point outside of the prefix.
    pub external_symlink_policy: ExternalSymlinkPolicy,
}

impl ArchiveLinkOptions {
    /// Default options to install packages for `platform` into `target_prefix`.
    pub fn new(target_prefix: impl Into<String>, platform: Subdir) -> Self {
        Self {
            target_prefix: target_prefix.into(),
            platform,
            python_info: None,
            clobber_mode: ClobberMode::default(),
            apple_codesign_behavior: AppleCodeSignBehavior::default(),
            external_symlink_policy: ExternalSymlinkPolicy::default(),
        }
    }
}

/// Where the files of all packages of an environment end up, see [`LinkPlan::new`].
#[derive(Debug, Clone)]
pub struct LinkPlan {
    packages: Vec<PlannedPackage>,
    clobbered_paths: HashMap<PathBuf, ClobberedPath>,
}

impl LinkPlan {
    /// Plans the installation of all `packages` of an environment into an empty prefix.
    ///
    /// The packages are sorted topologically. When multiple packages install the same file, the
    /// package that comes last in that order wins and the files of the other packages are moved
    /// into the `__clobbers__` directory, just like the regular installer does.
    pub fn new(
        packages: Vec<(RepoDataRecord, ArchivePackage)>,
        options: ArchiveLinkOptions,
    ) -> Result<Self, FromArchiveError> {
        let python_info = match options.python_info.clone() {
            Some(python_info) => Some(python_info),
            None => packages
                .iter()
                .find(|(record, _)| record.package_record.name.as_normalized() == "python")
                .map(|(record, _)| {
                    PythonInfo::from_python_record(&record.package_record, options.platform)
                })
                .transpose()
                .map_err(FromArchiveError::InvalidPythonInfo)?,
        };

        let packages = sort_topologically(packages);

        // Compute the location of every file in the prefix.
        let mut computed = Vec::with_capacity(packages.len());
        for (record, archive) in packages {
            let name = record.package_record.name.as_normalized().to_owned();
            let is_noarch_python = archive.index_json.noarch.is_python();
            if is_noarch_python && python_info.is_none() {
                return Err(FromArchiveError::MissingPythonInfo(name));
            }

            let mut paths = Vec::with_capacity(archive.paths_json.paths.len());
            for entry in &archive.paths_json.paths {
                if !is_contained_relative_path(&entry.relative_path) {
                    return Err(FromArchiveError::PathOutsideOfPrefix(
                        name,
                        entry.relative_path.clone(),
                    ));
                }
                let target_path = match (&python_info, is_noarch_python) {
                    (Some(python_info), true) => python_info
                        .get_python_noarch_target_path(&entry.relative_path)
                        .into_owned(),
                    _ => entry.relative_path.clone(),
                };
                if !is_contained_relative_path(&target_path) {
                    return Err(FromArchiveError::PathOutsideOfPrefix(name, target_path));
                }
                paths.push(PlannedPath {
                    entry: entry.clone(),
                    target_path,
                    original_path: None,
                });
            }

            let entry_points = match (&archive.link_json, &python_info) {
                (Some(link_json), Some(python_info)) if is_noarch_python => {
                    match &link_json.noarch {
                        NoArchLinks::Python(entry_points) => entry_points
                            .entry_points
                            .iter()
                            .map(|entry_point| {
                                python_entry_point_files(
                                    &options.target_prefix,
                                    entry_point,
                                    python_info,
                                    &options.platform,
                                )
                            })
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|e| {
                                FromArchiveError::FailedToCreateEntryPoint(name.clone(), e)
                            })?
                            .into_iter()
                            .flatten()
                            .collect(),
                        NoArchLinks::Generic => Vec::new(),
                    }
                }
                _ => Vec::new(),
            };

            computed.push(PlannedPackage {
                record,
                archive,
                paths,
                entry_points,
                options: LinkFileOptions {
                    target_prefix: options.target_prefix.clone(),
                    platform: options.platform,
                    apple_codesign_behavior: options.apple_codesign_behavior,
                    external_symlink_policy: options.external_symlink_policy,
                },
            });
        }

        // Move the files that lose against a file of another package into `__clobbers__`.
        let clobbers = resolve_clobbers(computed.iter().map(|package| {
            (
                &package.record.package_record.name,
                package.paths.iter().map(|path| path.target_path.as_path()),
            )
        }));
        if options.clobber_mode == ClobberMode::Error && !clobbers.clobbered_paths.is_empty() {
            return Err(FromArchiveError::ClobberingDetected(
                clobbers.clobbered_paths,
            ));
        }
        for (package, losing_paths) in computed.iter_mut().zip(clobbers.losing_paths) {
            if losing_paths.is_empty() {
                continue;
            }
            let name = package.record.package_record.name.as_normalized();
            for path in &mut package.paths {
                if losing_paths.contains(&path.target_path) {
                    let clobber_path = Path::new(CLOBBERS_DIR_NAME)
                        .join(name)
                        .join(&path.target_path);
                    path.original_path =
                        Some(std::mem::replace(&mut path.target_path, clobber_path));
                }
            }
        }

        Ok(Self {
            packages: computed,
            clobbered_paths: clobbers.clobbered_paths,
        })
    }

    /// The packages in topological order (dependencies first).
    pub fn packages(&self) -> &[PlannedPackage] {
        &self.packages
    }

    /// Consumes the plan and returns the packages in topological order (dependencies first).
    pub fn into_packages(self) -> Vec<PlannedPackage> {
        self.packages
    }

    /// The files that are installed by multiple packages.
    pub fn clobbered_paths(&self) -> &HashMap<PathBuf, ClobberedPath> {
        &self.clobbered_paths
    }
}

fn sort_topologically(
    packages: Vec<(RepoDataRecord, ArchivePackage)>,
) -> Vec<(RepoDataRecord, ArchivePackage)> {
    #[derive(Clone)]
    struct Indexed<'a>(usize, &'a PackageRecord);
    impl AsRef<PackageRecord> for Indexed<'_> {
        fn as_ref(&self) -> &PackageRecord {
            self.1
        }
    }

    let order: Vec<usize> = PackageRecord::sort_topologically(
        packages
            .iter()
            .enumerate()
            .map(|(index, (record, _))| Indexed(index, &record.package_record))
            .collect(),
    )
    .into_iter()
    .map(|Indexed(index, _)| index)
    .collect();

    let mut packages: Vec<_> = packages.into_iter().map(Some).collect();
    order
        .into_iter()
        .map(|index| packages[index].take().expect("indices are unique"))
        .collect()
}

#[derive(Debug, Clone)]
struct LinkFileOptions {
    target_prefix: String,
    platform: Subdir,
    apple_codesign_behavior: AppleCodeSignBehavior,
    external_symlink_policy: ExternalSymlinkPolicy,
}

#[derive(Debug, Clone)]
struct PlannedPath {
    entry: PathsEntry,
    /// The final location in the prefix.
    target_path: PathBuf,
    /// The location the file would have had if it had not been clobbered.
    original_path: Option<PathBuf>,
}

/// A single package of a [`LinkPlan`].
#[derive(Debug, Clone)]
pub struct PlannedPackage {
    record: RepoDataRecord,
    archive: ArchivePackage,
    paths: Vec<PlannedPath>,
    entry_points: Vec<EntryPointFile>,
    options: LinkFileOptions,
}

impl PlannedPackage {
    /// The record of the package.
    pub fn record(&self) -> &RepoDataRecord {
        &self.record
    }

    /// The archive of the package.
    pub fn archive(&self) -> &ArchivePackage {
        &self.archive
    }

    /// The locations (relative to the prefix) of all files this package installs, including its
    /// entry points but excluding its `conda-meta` record.
    pub fn target_paths(&self) -> impl Iterator<Item = &Path> {
        self.paths
            .iter()
            .map(|path| path.target_path.as_path())
            .chain(
                self.entry_points
                    .iter()
                    .map(|file| file.relative_path.as_path()),
            )
    }

    /// Streams the files of the package from its archive into `sink`, replacing prefix
    /// placeholders on the way, and returns the [`PrefixRecord`] that describes the installed
    /// package.
    ///
    /// The `conda-meta` record itself is not passed to the sink, write
    /// [`PrefixRecord::file_name`] in `conda-meta/` with [`PrefixRecord::write_to`] for that.
    pub fn link(&self, mut sink: impl PrefixSink) -> Result<PrefixRecord, FromArchiveError> {
        let name = self.record.package_record.name.as_normalized();
        let by_archive_path: HashMap<&Path, usize> = self
            .paths
            .iter()
            .enumerate()
            .map(|(index, path)| (path.entry.relative_path.as_path(), index))
            .collect();

        let mut results: Vec<Option<prefix_record::PathsEntry>> = vec![None; self.paths.len()];
        // Hard links in the archive refer to another entry of the archive. Their contents are
        // copied in a second pass over the archive.
        let mut hard_links: HashMap<PathBuf, Vec<usize>> = HashMap::new();

        self.archive.for_each_content_entry(|entry_path, entry| {
            let Some(&index) = by_archive_path.get(entry_path) else {
                // The regular installer also only installs the files in `paths.json`.
                return Ok(());
            };
            if results[index].is_some() {
                return Ok(());
            }
            let planned = &self.paths[index];
            let entry_type = entry.header().entry_type();
            let linked = match entry_type {
                tar::EntryType::Regular | tar::EntryType::Continuous => {
                    let mode = entry_mode(entry, 0o644);
                    let size = entry.size();
                    self.link_file(planned, mode, size, entry, &mut sink)?
                }
                tar::EntryType::Symlink => {
                    let target = entry
                        .link_name()
                        .map_err(|e| read_error(&self.archive, e))?
                        .ok_or_else(|| {
                            FromArchiveError::UnsupportedEntryType(
                                name.to_owned(),
                                entry_path.to_path_buf(),
                            )
                        })?
                        .into_owned();
                    self.link_symlink(planned, &target, &mut sink)?
                }
                tar::EntryType::Link => {
                    let target = entry
                        .link_name()
                        .map_err(|e| read_error(&self.archive, e))?
                        .ok_or_else(|| {
                            FromArchiveError::UnsupportedEntryType(
                                name.to_owned(),
                                entry_path.to_path_buf(),
                            )
                        })?;
                    hard_links
                        .entry(normalize_entry_path(&target))
                        .or_default()
                        .push(index);
                    return Ok(());
                }
                tar::EntryType::Directory if planned.entry.path_type == PathType::Directory => {
                    let mode = entry_mode(entry, 0o755);
                    Self::link_directory(planned, mode, &mut sink)?
                }
                tar::EntryType::Directory => return Ok(()),
                _ => {
                    return Err(FromArchiveError::UnsupportedEntryType(
                        name.to_owned(),
                        entry_path.to_path_buf(),
                    ));
                }
            };
            results[index] = Some(linked);
            Ok(())
        })?;

        if !hard_links.is_empty() {
            self.archive.for_each_content_entry(|entry_path, entry| {
                let Some(indices) = hard_links.get(entry_path) else {
                    return Ok(());
                };
                if !matches!(
                    entry.header().entry_type(),
                    tar::EntryType::Regular | tar::EntryType::Continuous
                ) {
                    return Err(FromArchiveError::UnsupportedEntryType(
                        name.to_owned(),
                        entry_path.to_path_buf(),
                    ));
                }
                let mode = entry_mode(entry, 0o644);
                let size = entry.size();
                // Every link gets its own copy of the contents, so the contents are buffered
                // when there is more than one link to the same file.
                if let [index] = indices.as_slice() {
                    let linked =
                        self.link_file(&self.paths[*index], mode, size, entry, &mut sink)?;
                    results[*index] = Some(linked);
                } else {
                    let mut contents = Vec::new();
                    entry
                        .read_to_end(&mut contents)
                        .map_err(|e| read_error(&self.archive, e))?;
                    for &index in indices {
                        let linked = self.link_file(
                            &self.paths[index],
                            mode,
                            size,
                            &mut contents.as_slice(),
                            &mut sink,
                        )?;
                        results[index] = Some(linked);
                    }
                }
                Ok(())
            })?;
        }

        let mut paths = Vec::with_capacity(results.len() + self.entry_points.len());
        for (planned, result) in self.paths.iter().zip(results) {
            match result {
                Some(result) => paths.push(result),
                // Empty directories are not always stored in the archive.
                None if planned.entry.path_type == PathType::Directory => {
                    paths.push(Self::link_directory(planned, 0o755, &mut sink)?);
                }
                None => {
                    return Err(FromArchiveError::MissingFile(
                        name.to_owned(),
                        planned.entry.relative_path.clone(),
                    ));
                }
            }
        }

        for file in &self.entry_points {
            sink.add_file(
                &file.relative_path,
                file.mode,
                file.contents.len() as u64,
                &mut file.contents.as_ref(),
            )
            .map_err(|e| FromArchiveError::SinkError(file.relative_path.clone(), e))?;
            paths.push(file.paths_entry());
        }

        paths.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        Ok(PrefixRecord::from_repodata_record(
            self.record.clone(),
            paths,
        ))
    }

    fn link_file(
        &self,
        planned: &PlannedPath,
        mode: u32,
        size: u64,
        contents: &mut dyn Read,
        sink: &mut impl PrefixSink,
    ) -> Result<prefix_record::PathsEntry, FromArchiveError> {
        let target_path = &planned.target_path;
        let sink_error = |e| FromArchiveError::SinkError(target_path.clone(), e);

        let Some(placeholder) = &planned.entry.prefix_placeholder else {
            // The file is passed through unchanged. Its hash is only computed if `paths.json`
            // does not contain it.
            let sha256 = if let Some(sha256) = planned.entry.sha256 {
                let mut contents = contents.take(size);
                sink.add_file(target_path, mode, size, &mut contents)
                    .map_err(sink_error)?;
                io::copy(&mut contents, &mut io::sink())
                    .map_err(|e| read_error(&self.archive, e))?;
                sha256
            } else {
                let mut contents = HashingReader::<_, Sha256>::new(contents.take(size));
                sink.add_file(target_path, mode, size, &mut contents)
                    .map_err(sink_error)?;
                io::copy(&mut contents, &mut io::sink())
                    .map_err(|e| read_error(&self.archive, e))?;
                contents.finalize().1
            };
            return Ok(Self::paths_entry(planned, sha256, size, None));
        };

        let mut source = Vec::with_capacity(usize::try_from(size).unwrap_or_default());
        contents
            .take(size)
            .read_to_end(&mut source)
            .map_err(|e| read_error(&self.archive, e))?;

        let mut patched = Vec::with_capacity(source.len());
        replace_prefix_placeholder(
            &source,
            &mut patched,
            &planned.entry.relative_path,
            &placeholder.placeholder,
            placeholder.experimental_offsets.as_ref(),
            placeholder.file_mode,
            &self.options.target_prefix,
            self.options.platform,
        )
        .map_err(|e| FromArchiveError::FailedToLink(planned.entry.relative_path.clone(), e))?;
        let sha256 = rattler_digest::compute_bytes_digest::<Sha256>(&patched);

        // Modified macOS binaries have to be signed again, which requires `codesign`.
        if (self.options.platform.is_osx() || self.options.platform.is_ios())
            && placeholder.file_mode == FileMode::Binary
            && (mode & 0o111 != 0 || FileType::detect(&source) == Some(FileType::MachO))
            && planned
                .entry
                .sha256
                .is_some_and(|original| original != sha256)
        {
            match self.options.apple_codesign_behavior {
                AppleCodeSignBehavior::Fail => {
                    return Err(FromArchiveError::CodeSigningRequired(target_path.clone()));
                }
                AppleCodeSignBehavior::Ignore => tracing::warn!(
                    "'{}' needs to be code signed after prefix replacement, which is not \
                     supported when linking from an archive",
                    target_path.display()
                ),
                AppleCodeSignBehavior::DoNothing => {}
            }
        }

        sink.add_file(
            target_path,
            mode,
            patched.len() as u64,
            &mut patched.as_slice(),
        )
        .map_err(sink_error)?;
        Ok(Self::paths_entry(
            planned,
            sha256,
            patched.len() as u64,
            Some(placeholder.file_mode),
        ))
    }

    fn link_symlink(
        &self,
        planned: &PlannedPath,
        target: &Path,
        sink: &mut impl PrefixSink,
    ) -> Result<prefix_record::PathsEntry, FromArchiveError> {
        let target_path = &planned.target_path;
        if symlink_escapes_prefix(target_path, target) {
            match self.options.external_symlink_policy {
                ExternalSymlinkPolicy::Allow => {}
                ExternalSymlinkPolicy::Warn => tracing::warn!(
                    "symlink {} points outside the target prefix: {}",
                    target_path.display(),
                    target.display()
                ),
                ExternalSymlinkPolicy::Deny => {
                    return Err(FromArchiveError::SymlinkTargetEscapesPrefix(
                        target_path.clone(),
                    ));
                }
            }
        }
        sink.add_symlink(target_path, target)
            .map_err(|e| FromArchiveError::SinkError(target_path.clone(), e))?;

        // Like the regular installer, hash the target of the symlink instead of the file it
        // points to.
        let sha256 =
            rattler_digest::compute_bytes_digest::<Sha256>(target.to_string_lossy().as_bytes());
        let size = planned.entry.size_in_bytes.unwrap_or_default();
        Ok(Self::paths_entry(planned, sha256, size, None))
    }

    fn link_directory(
        planned: &PlannedPath,
        mode: u32,
        sink: &mut impl PrefixSink,
    ) -> Result<prefix_record::PathsEntry, FromArchiveError> {
        sink.add_directory(&planned.target_path, mode)
            .map_err(|e| FromArchiveError::SinkError(planned.target_path.clone(), e))?;
        let size = planned.entry.size_in_bytes.unwrap_or_default();
        Ok(Self::paths_entry(
            planned,
            Sha256Hash::default(),
            size,
            None,
        ))
    }

    fn paths_entry(
        planned: &PlannedPath,
        sha256: Sha256Hash,
        size: u64,
        file_mode: Option<FileMode>,
    ) -> prefix_record::PathsEntry {
        let entry = &planned.entry;
        prefix_record::PathsEntry {
            relative_path: planned.target_path.clone(),
            original_path: planned.original_path.clone(),
            path_type: entry.path_type.into(),
            no_link: entry.no_link,
            sha256: entry.sha256,
            // Only set sha256_in_prefix if it differs from the original sha256
            sha256_in_prefix: if Some(sha256) == entry.sha256 {
                None
            } else {
                Some(sha256)
            },
            size_in_bytes: Some(size),
            file_mode,
            prefix_placeholder: entry
                .prefix_placeholder
                .as_ref()
                .map(|p| p.placeholder.clone()),
        }
    }
}

fn read_error(archive: &ArchivePackage, error: io::Error) -> FromArchiveError {
    FromArchiveError::FailedToReadArchive(archive.path.clone(), Box::new(error.into()))
}

/// The permission bits of a tar entry, or `default` if the header does not contain them.
fn entry_mode(entry: &tar::Entry<'_, impl Read>, default: u32) -> u32 {
    entry.header().mode().map_or(default, |mode| mode & 0o7777)
}

#[cfg(test)]
mod tests;
