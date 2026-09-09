//! Plans and links an extracted package's files by destination directory.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use itertools::Itertools;
use rattler_conda_types::{
    Platform,
    package::{IndexJson, LinkJson, NoArchLinks, PackageFile, PathsEntry, PathsJson},
    prefix::Prefix,
    prefix_record::{self, LinkType},
};
use rayon::{
    iter::Either,
    prelude::{IndexedParallelIterator, IntoParallelIterator, ParallelIterator},
};
use tracing::instrument;

use super::{
    AppleCodeSignBehavior, ExternalSymlinkPolicy, InstallError, InstallOptions, LinkMethod,
    TransactionLinkContext,
    clobber_registry::CLOBBERS_DIR_NAME,
    compute_paths,
    entry_point::{create_unix_python_entry_point, create_windows_python_entry_point},
    filesystem::{
        DirectoryCreationMode, can_create_hardlinks_sync, can_create_reflinks_sync,
        can_create_symlinks_sync,
    },
    link_file, modification_time,
};

#[derive(Debug)]
pub(super) struct LinkPath {
    pub(super) entry: PathsEntry,
    pub(super) computed_path: PathBuf,
    pub(super) clobber_path: Option<PathBuf>,
}

impl LinkPath {
    fn destination_parent(&self) -> Option<&Path> {
        self.clobber_path
            .as_deref()
            .unwrap_or(self.computed_path.as_path())
            .parent()
    }
}

/// Groups a package's files by destination and records its directory prerequisites.
struct PackageLinkPlan {
    directories_to_construct: HashSet<PathBuf>,
    paths_by_directory: HashMap<PathBuf, Vec<LinkPath>>,
}

impl PackageLinkPlan {
    fn new(
        link_paths: impl IntoIterator<Item = LinkPath>,
        directory_creation_mode: DirectoryCreationMode,
    ) -> Self {
        let mut directories_to_construct = HashSet::new();
        let mut paths_by_directory = HashMap::new();

        for link_path in link_paths {
            let Some(entry_parent) = link_path.computed_path.parent() else {
                continue;
            };

            let destination_parent = match directory_creation_mode {
                DirectoryCreationMode::Barrier => {
                    Self::add_directory_ancestors(
                        &mut directories_to_construct,
                        Some(entry_parent),
                    );
                    Self::add_directory_ancestors(
                        &mut directories_to_construct,
                        link_path
                            .clobber_path
                            .as_deref()
                            .and_then(|path| path.parent()),
                    );
                    Some(entry_parent)
                }
                DirectoryCreationMode::Overlap => link_path.destination_parent(),
            };

            if let Some(destination_parent) = destination_parent {
                paths_by_directory
                    .entry(destination_parent.to_path_buf())
                    .or_insert_with(Vec::new)
                    .push(link_path);
            }
        }

        Self {
            directories_to_construct,
            paths_by_directory,
        }
    }

    fn add_directory_ancestors(
        directories_to_construct: &mut HashSet<PathBuf>,
        mut current_path: Option<&Path>,
    ) {
        while let Some(path) = current_path {
            if path.as_os_str().is_empty() || !directories_to_construct.insert(path.to_path_buf()) {
                break;
            }
            current_path = path.parent();
        }
    }

    fn into_directory_link_groups(self) -> Vec<DirectoryLinkGroup> {
        let mut link_groups = self
            .paths_by_directory
            .into_iter()
            .map(|(directory, link_paths)| DirectoryLinkGroup {
                directory,
                link_paths,
                path_entries: Ok(Vec::new()),
            })
            .collect_vec();
        link_groups.sort_unstable_by(|left, right| {
            left.directory
                .components()
                .count()
                .cmp(&right.directory.components().count())
                .then_with(|| right.link_paths.len().cmp(&left.link_paths.len()))
        });
        link_groups
    }
}

/// Work and result storage for one destination directory.
struct DirectoryLinkGroup {
    directory: PathBuf,
    link_paths: Vec<LinkPath>,
    // Written by the scoped Rayon task after the directory becomes available.
    path_entries: Result<Vec<prefix_record::PathsEntry>, InstallError>,
}

/// Links planned paths using one package's immutable linking configuration.
struct PackageLinker<'a> {
    package_dir: &'a Path,
    target_dir: &'a Prefix,
    target_prefix: &'a str,
    allow_symbolic_links: bool,
    allow_hard_links: bool,
    allow_ref_links: bool,
    platform: Platform,
    apple_codesign_behavior: AppleCodeSignBehavior,
    modification_time: filetime::FileTime,
    external_symlink_policy: ExternalSymlinkPolicy,
}

impl PackageLinker<'_> {
    fn link_paths(
        &self,
        link_paths: Vec<LinkPath>,
    ) -> Result<Vec<prefix_record::PathsEntry>, InstallError> {
        let mut path_entries = Vec::with_capacity(link_paths.len());
        for link_path in link_paths {
            let LinkPath {
                entry,
                computed_path,
                clobber_path,
            } = link_path;
            let (destination, original_path) = match clobber_path {
                Some(destination) => (destination, Some(computed_path)),
                None => (computed_path, None),
            };
            let link_result = link_file(
                &entry,
                destination,
                self.package_dir,
                self.target_dir,
                self.target_prefix,
                self.allow_symbolic_links && !entry.no_link,
                self.allow_hard_links && !entry.no_link,
                self.allow_ref_links && !entry.no_link,
                self.platform,
                self.apple_codesign_behavior,
                self.modification_time,
                self.external_symlink_policy,
            );

            let result = match link_result {
                Ok(Some(linked_file)) => linked_file,
                Ok(None) => continue,
                Err(error) => {
                    return Err(InstallError::FailedToLink(entry.relative_path, error));
                }
            };

            path_entries.push(prefix_record::PathsEntry {
                relative_path: result.relative_path,
                original_path,
                path_type: entry.path_type.into(),
                no_link: entry.no_link,
                sha256: entry.sha256,
                sha256_in_prefix: if Some(result.sha256) == entry.sha256 {
                    None
                } else {
                    Some(result.sha256)
                },
                size_in_bytes: Some(result.file_size),
                file_mode: match result.method {
                    LinkMethod::Patched(file_mode) => Some(file_mode),
                    LinkMethod::Reflink
                    | LinkMethod::Hardlink
                    | LinkMethod::Softlink
                    | LinkMethod::Copy => None,
                },
                prefix_placeholder: entry
                    .prefix_placeholder
                    .map(|placeholder| placeholder.placeholder),
            });
        }

        Ok(path_entries)
    }

    fn execute_overlapped(
        &self,
        link_plan: PackageLinkPlan,
    ) -> Result<Vec<prefix_record::PathsEntry>, InstallError> {
        let mut link_groups = link_plan.into_directory_link_groups();
        let mut directory_error = None;
        rayon::in_place_scope(|scope| {
            for link_group in &mut link_groups {
                let full_path = self.target_dir.path().join(&link_group.directory);
                if let Err(error) = fs::create_dir_all(&full_path) {
                    directory_error = Some(InstallError::FailedToCreateDirectory(full_path, error));
                    break;
                }

                let link_paths = std::mem::take(&mut link_group.link_paths);
                let path_entries = &mut link_group.path_entries;
                scope.spawn(move |_| {
                    *path_entries = self.link_paths(link_paths);
                });
            }
        });
        if let Some(error) = directory_error {
            return Err(error);
        }

        let mut paths = Vec::new();
        for link_group in link_groups {
            paths.extend(link_group.path_entries?);
        }
        Ok(paths)
    }

    fn execute_with_directory_barrier(
        mut self,
        link_plan: PackageLinkPlan,
        is_noarch_python: bool,
    ) -> Result<Vec<prefix_record::PathsEntry>, InstallError> {
        let PackageLinkPlan {
            directories_to_construct,
            mut paths_by_directory,
        } = link_plan;
        let mut created_directories = HashSet::new();
        let mut reflinked_files = HashMap::new();
        for directory in directories_to_construct
            .into_iter()
            .sorted_by(|left, right| left.components().count().cmp(&right.components().count()))
        {
            let full_path = self.target_dir.path().join(&directory);

            if created_directories
                .iter()
                .any(|created| directory.starts_with(created))
            {
                continue;
            }

            if full_path.exists() {
                continue;
            }

            if self.allow_ref_links
                && cfg!(target_os = "macos")
                && !directory.starts_with(CLOBBERS_DIR_NAME)
                && !is_noarch_python
            {
                match reflink_copy::reflink(self.package_dir.join(&directory), &full_path) {
                    Ok(_) => {
                        created_directories.insert(directory.clone());
                        let (matching, non_matching): (HashMap<_, _>, HashMap<_, _>) =
                            paths_by_directory
                                .drain()
                                .partition(|(path, _)| path.starts_with(&directory));
                        reflinked_files.extend(matching);
                        paths_by_directory = non_matching;
                    }
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => (),
                    Err(_) => {
                        self.allow_ref_links = false;
                        match fs::create_dir(&full_path) {
                            Ok(()) => {}
                            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                            Err(error) => {
                                return Err(InstallError::FailedToCreateDirectory(
                                    full_path, error,
                                ));
                            }
                        }
                    }
                }
            } else {
                match fs::create_dir(&full_path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(InstallError::FailedToCreateDirectory(full_path, error));
                    }
                }
            }
        }

        let mut reflinked_paths_entries = Vec::new();
        for (parent_dir, files) in reflinked_files {
            for link_path in files {
                if link_path.clobber_path.is_some() || link_path.entry.prefix_placeholder.is_some()
                {
                    paths_by_directory
                        .entry(parent_dir.clone())
                        .or_insert_with(Vec::new)
                        .push(link_path);
                } else {
                    let entry = link_path.entry;
                    reflinked_paths_entries.push(prefix_record::PathsEntry {
                        relative_path: entry.relative_path,
                        path_type: entry.path_type.into(),
                        no_link: entry.no_link,
                        sha256: entry.sha256,
                        size_in_bytes: entry.size_in_bytes,
                        original_path: None,
                        sha256_in_prefix: None,
                        file_mode: None,
                        prefix_placeholder: None,
                    });
                }
            }
        }

        let linked_path_groups = paths_by_directory
            .into_values()
            .collect_vec()
            .into_par_iter()
            .with_min_len(100)
            .map(|link_paths| self.link_paths(link_paths))
            .collect::<Result<Vec<_>, _>>()?;
        let mut paths = linked_path_groups.into_iter().flatten().collect_vec();
        paths.extend(reflinked_paths_entries);
        Ok(paths)
    }
}

/// Links one extracted package into `target_dir` on the current thread.
///
/// Share the same [`TransactionLinkContext`] across packages in a transaction.
/// Returned entries are sorted by relative path for reproducible prefix records.
#[instrument(skip_all, fields(package_dir = % package_dir.display()))]
pub fn link_package_sync(
    package_dir: &Path,
    target_dir: &Prefix,
    link_context: &TransactionLinkContext,
    options: InstallOptions,
) -> Result<(Vec<prefix_record::PathsEntry>, LinkType), InstallError> {
    // Determine the target prefix for linking
    let target_prefix = options
        .target_prefix
        .as_deref()
        .unwrap_or(target_dir)
        .to_str()
        .ok_or(InstallError::TargetPrefixIsNotUtf8)?
        .to_owned();

    // Reuse or read the `paths.json` and `index.json` files from the package
    // directory
    let paths_json = options.paths_json.map_or_else(
        || {
            PathsJson::from_package_directory_with_deprecated_fallback(package_dir)
                .map_err(InstallError::FailedToReadPathsJson)
        },
        Ok,
    )?;
    let index_json = options.index_json.map_or_else(
        || {
            IndexJson::from_package_directory(package_dir)
                .map_err(InstallError::FailedToReadIndexJson)
        },
        Ok,
    )?;
    let modification_time = modification_time(package_dir);

    // Error out if this is a noarch python package but the python information is
    // missing.
    if index_json.noarch.is_python() && options.python_info.is_none() {
        return Err(InstallError::MissingPythonInfo);
    }

    // Parse the `link.json` file and extract entry points from it.
    let link_json = if index_json.noarch.is_python() {
        options.link_json.flatten().map_or_else(
            || {
                LinkJson::from_package_directory(package_dir)
                    .map_or_else(
                        |e| {
                            // Its ok if the file is not present.
                            if e.kind() == ErrorKind::NotFound {
                                Ok(None)
                            } else {
                                Err(e)
                            }
                        },
                        |link_json| Ok(Some(link_json)),
                    )
                    .map_err(InstallError::FailedToReadLinkJson)
            },
            |value| Ok(Some(value)),
        )?
    } else {
        None
    };

    // Determine whether or not we can use symbolic links
    let allow_symbolic_links = options
        .allow_symbolic_links
        .unwrap_or_else(|| can_create_symlinks_sync(target_dir));
    let allow_hard_links = options
        .allow_hard_links
        .unwrap_or_else(|| can_create_hardlinks_sync(target_dir, package_dir));
    // Record the link type that will be used for this package. Hard links take
    // priority
    let link_type = if allow_hard_links {
        LinkType::HardLink
    } else {
        LinkType::Copy
    };
    let allow_ref_links = options
        .allow_ref_links
        .unwrap_or_else(|| can_create_reflinks_sync(target_dir, package_dir, allow_hard_links));

    // Determine the platform to use
    let platform = options.platform.unwrap_or(Platform::current());

    // compute all path renames
    let final_paths = compute_paths(&index_json, &paths_json, options.python_info.as_ref());

    // Register all paths in the transaction's path registry.
    let clobber_paths = link_context
        .clobber_registry()
        .register_paths(&index_json, &final_paths);

    let final_paths = final_paths.into_iter().map(|el| {
        let (entry, computed_path) = el;
        let clobber_path = clobber_paths.get(&computed_path).cloned();
        LinkPath {
            entry,
            computed_path,
            clobber_path,
        }
    });

    let directory_creation_mode = DirectoryCreationMode::for_target(target_dir.path());
    let link_plan = PackageLinkPlan::new(final_paths, directory_creation_mode);
    let package_linker = PackageLinker {
        package_dir,
        target_dir,
        target_prefix: &target_prefix,
        allow_symbolic_links,
        allow_hard_links,
        allow_ref_links,
        platform,
        apple_codesign_behavior: options.apple_codesign_behavior,
        modification_time,
        external_symlink_policy: options.external_symlink_policy,
    };
    let mut paths = match directory_creation_mode {
        DirectoryCreationMode::Barrier => package_linker
            .execute_with_directory_barrier(link_plan, index_json.noarch.is_python())?,
        DirectoryCreationMode::Overlap => package_linker.execute_overlapped(link_plan)?,
    };

    let python_info = options.python_info;

    // If this package is a noarch python package we also have to create entry
    // points.
    if let Some(link_json) = link_json {
        // Parse the `link.json` file and extract entry points from it.
        let entry_points = match link_json.noarch {
            NoArchLinks::Python(entry_points) => entry_points.entry_points,
            NoArchLinks::Generic => {
                unreachable!("we only use link.json for noarch: python packages")
            }
        };

        // Get python info
        let python_info = python_info
            .expect("should be safe because its checked above that this contains a value");

        // Create entry points for each listed item. This is different between Windows
        // and unix because on Windows, two PathEntry's are created whereas on
        // Linux only one is created.
        let mut entry_point_paths = if platform.is_windows() {
            entry_points
                .into_iter()
                .flat_map(move |entry_point| {
                    match create_windows_python_entry_point(
                        target_dir,
                        &target_prefix,
                        &entry_point,
                        &python_info,
                        &platform,
                    ) {
                        Ok([a, b]) => Either::Left([Ok(a), Ok(b)].into_iter()),
                        Err(e) => Either::Right(std::iter::once(Err(
                            InstallError::FailedToCreatePythonEntryPoint(e),
                        ))),
                    }
                })
                .collect::<Result<Vec<_>, _>>()?
        } else {
            entry_points
                .into_iter()
                .map(move |entry_point| {
                    match create_unix_python_entry_point(
                        target_dir,
                        &target_prefix,
                        &entry_point,
                        &python_info,
                    ) {
                        Ok(a) => Ok(a),
                        Err(e) => Err(InstallError::FailedToCreatePythonEntryPoint(e)),
                    }
                })
                .collect::<Result<_, _>>()?
        };

        paths.append(&mut entry_point_paths);
    };

    paths.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok((paths, link_type))
}
