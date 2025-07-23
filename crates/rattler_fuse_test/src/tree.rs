use rattler::install::compute_paths;
use rattler::install::PythonInfo;
use rattler_cache::package_cache::PackageCache;
use rattler_conda_types::package::{
    ArchiveIdentifier, EntryPoint, FileMode, IndexJson, LinkJson, NoArchLinks, PackageFile,
    PathType, PathsJson,
};
use rattler_conda_types::{PackageRecord, Platform};
use rattler_lock::LockFile;
use rattler_lock::LockedPackageRef;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::tree_objects::{
    Directory, EntryPoint as EntryPoint2, File, Node, NodeRef, NodeWeak, NotADirectoryError,
    PatchMode, Symlink,
};

// Static atomic counter for generating unique inode numbers
static NEXT_INODE: AtomicU64 = AtomicU64::new(1); // Start at 1 as 0 is often reserved

// Function to get the next available inode number
fn get_next_inode() -> u64 {
    NEXT_INODE.fetch_add(5, Ordering::Relaxed)
}

/// A filesystem tree with an inode lookup table
pub struct EnvTree {
    root: NodeRef,
    /// Maps inodes to their corresponding nodes for O(1) lookup
    inode_map: HashMap<u64, NodeWeak>,
}

impl EnvTree {
    pub async fn from_lock_file(
        path: &Path,
        env_name: &str,
        target_dir: &Path,
        cache_dir: &PathBuf,
    ) -> Result<EnvTree, Box<dyn std::error::Error>> {
        let target_dir = Rc::new(target_dir.to_string_lossy().as_bytes().to_vec());
        let package_cache = PackageCache::new(cache_dir);
        let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());

        let lock_file = LockFile::from_path(path).unwrap();
        let environment = lock_file.environment(env_name).unwrap();
        let target_platform = Platform::current();
        let packages: Vec<_> = environment
            .packages(target_platform)
            .expect("the platform for which the explicit lock file was created does not match the current platform")
            .collect();

        let python_info = find_python_info(
            packages
                .iter()
                .filter_map(|p| p.as_conda().map(rattler_lock::CondaPackageData::record)),
            target_platform,
        ).map(Rc::new);

        let mut tree = EnvTree::new();
        for package in packages {
            match package {
                LockedPackageRef::Conda(p) => {
                    let url: &reqwest::Url = p.location().as_url().unwrap();
                    let package_info = ArchiveIdentifier::try_from_url(url).unwrap();
                    // // TODO: crates/rattler/src/install/installer/mod.rs:409
                    let package_cache_lock = package_cache
                        .get_or_fetch_from_url(package_info, url.clone(), client.clone(), None)
                        .await
                        .unwrap();
                    let package_dir = package_cache_lock.path();

                    let paths_json =
                        PathsJson::from_package_directory_with_deprecated_fallback(package_dir)
                            .unwrap();
                    let index_json = IndexJson::from_package_directory(package_dir).unwrap();

                    // Error out if this is a noarch python package but the python information is
                    // missing.
                    assert!(!(index_json.noarch.is_python() && python_info.is_none()), "No python information found for noarch python package");

                    let link_json = if index_json.noarch.is_python() {
                        Some(LinkJson::from_package_directory(package_dir).unwrap())
                    } else {
                        None
                    };

                    let final_paths =
                        compute_paths(&index_json, &paths_json, python_info.as_deref());

                    for (entry, computed_path) in final_paths {
                        let target = package_dir.join(&entry.relative_path);
                        match entry.path_type {
                            PathType::HardLink => {
                                let patch_mode = match entry.prefix_placeholder {
                                    Some(placeholder) => match placeholder.file_mode {
                                        FileMode::Binary => PatchMode::Binary(
                                            placeholder.placeholder.clone().into_bytes(),
                                            target_dir.clone(),
                                            target_platform,
                                        ),
                                        FileMode::Text => PatchMode::Text(
                                            placeholder.placeholder.clone().into_bytes(),
                                            target_dir.clone(),
                                            target_platform,
                                        ),
                                    },
                                    None => PatchMode::None,
                                };
                                tree.add_file(
                                    &computed_path,
                                    target,
                                    entry.size_in_bytes.unwrap(),
                                    patch_mode,
                                )?;
                            }
                            PathType::SoftLink => {
                                tree.add_symlink(&computed_path, target)?;
                            }
                            PathType::Directory => panic!("Directory: {path:?}"),
                        }
                    }

                    // If this package is a noarch python package we also have to create entry points.
                    if let Some(link_json) = link_json {
                        let entry_points = match link_json.noarch {
                            NoArchLinks::Python(entry_points) => entry_points.entry_points,
                            NoArchLinks::Generic => {
                                unreachable!("we only use link.json for noarch: python packages")
                            }
                        };

                        let python_info = python_info.clone().expect(
                            "should be safe because its checked above that this contains a value",
                        );

                        for entry_point in entry_points {
                            tree.add_python_entry_point(
                                &target_dir,
                                &target_platform,
                                &entry_point,
                                &python_info,
                            )?;
                        }
                    }
                }

                LockedPackageRef::Pypi(p, pe) => {
                    panic!("Pypi package: {} {:?}", p.location, pe.extras);
                }
            }
        }

        println!("Tree has {} nodes", tree.inode_map.len());

        Ok(tree)
    }

    fn new() -> Self {
        let ino = get_next_inode();
        let root: NodeRef = Rc::new(RefCell::new(Node::Directory(Directory::new(
            ino,
            "ROOT".into(),
            None,
        ))));

        let mut inode_map = HashMap::new();
        inode_map.insert(ino, Rc::downgrade(&root));

        EnvTree {
            root,
            inode_map,
        }
    }

    fn get_directory(&mut self, path: &Path) -> Result<NodeRef, NotADirectoryError> {
        let mut parent = Rc::clone(&self.root);

        for component in path.iter() {
            let child = match &mut *parent.borrow_mut() {
                Node::Directory(dir) => {
                    if let Some(child) = dir.get_child(component) {
                        Rc::clone(&child)
                    } else {
                        let ino = get_next_inode();
                        let new_node = Rc::new(RefCell::new(Node::Directory(Directory::new(
                            ino,
                            component.into(),
                            Some(Rc::downgrade(&parent)),
                        ))));
                        self.inode_map.insert(ino, Rc::downgrade(&new_node));
                        dir.add_child(Rc::clone(&new_node));
                        new_node
                    }
                }
                _ => return Err(NotADirectoryError),
            };
            parent = child;
        }

        Ok(parent)
    }

    pub fn add_file(
        &mut self,
        path: &Path,
        target: PathBuf,
        size: u64,
        patch_mode: PatchMode,
    ) -> Result<(), NotADirectoryError> {
        let parent_path = path
            .parent()
            .expect("File path must have a parent directory");
        let file_name = path.file_name().expect("File path must have a basename");

        let parent = self.get_directory(&parent_path.to_path_buf())?;
        let ino = get_next_inode();
        let new_node = Rc::new(RefCell::new(Node::File(File::new(
            ino,
            file_name.into(),
            Rc::downgrade(&parent),
            target,
            size,
            patch_mode,
        ))));
        self.inode_map.insert(ino, Rc::downgrade(&new_node));

        match &mut *parent.borrow_mut() {
            Node::Directory(dir) => {
                dir.add_child(Rc::clone(&new_node));
            }
            _ => unreachable!(),
        };

        Ok(())
    }

    pub fn add_symlink(
        &mut self,
        path: &Path,
        target: PathBuf,
    ) -> Result<(), NotADirectoryError> {
        let parent_path = path
            .parent()
            .expect("Symlink path must have a parent directory");
        let symlink_name = path.file_name().expect("Symlink path must have a basename");

        let parent = self.get_directory(&parent_path.to_path_buf())?;
        let ino = get_next_inode();
        let new_node = Rc::new(RefCell::new(Node::Symlink(Symlink::new(
            ino,
            symlink_name.into(),
            Rc::downgrade(&parent),
            target,
        ))));
        self.inode_map.insert(ino, Rc::downgrade(&new_node));

        match &mut *parent.borrow_mut() {
            Node::Directory(dir) => {
                dir.add_child(Rc::clone(&new_node));
            }
            _ => unreachable!(),
        };

        Ok(())
    }

    pub fn add_python_entry_point(
        &mut self,
        target_prefix: &Rc<Vec<u8>>,
        target_plaform: &Platform,
        entry_point: &EntryPoint,
        python_info: &Rc<PythonInfo>,
    ) -> Result<(), NotADirectoryError> {
        if target_plaform.is_windows() {
            todo!("Windows entry points not yet supported");
        }

        let parent = self.get_directory(&python_info.bin_dir.clone())?;
        let ino = get_next_inode();
        let new_node = Rc::new(RefCell::new(Node::EntryPoint(EntryPoint2::new(
            ino,
            OsString::from(entry_point.command.clone()),
            Rc::downgrade(&parent),
            entry_point.module.clone(),
            entry_point.function.clone(),
            Rc::clone(target_prefix),
            Rc::clone(python_info),
        ))));
        self.inode_map.insert(ino, Rc::downgrade(&new_node));

        match &mut *parent.borrow_mut() {
            Node::Directory(dir) => {
                dir.add_child(Rc::clone(&new_node));
            }
            _ => unreachable!(),
        };

        Ok(())
    }

    pub fn find_by_inode(&self, ino: u64) -> Option<NodeRef> {
        let node = self.inode_map.get(&ino)?.upgrade()?;
        Some(node)
    }

    pub fn root_ino(&self) -> u64 {
        self.root.borrow().ino()
    }

    pub fn print_tree(&self) {
        self.root.borrow().print_tree(0);
    }
}

/// Determine the version of Python used by a set of packages. Returns `None` if
/// none of the packages refers to a Python installation.
fn find_python_info(
    records: impl IntoIterator<Item = impl AsRef<PackageRecord>>,
    platform: Platform,
) -> Option<PythonInfo> {
    records
        .into_iter()
        .find(|r| is_python_record(r.as_ref()))
        .map(|record| PythonInfo::from_python_record(record.as_ref(), platform))
        .map_or(Ok(None), |info| info.map(Some))
        .unwrap()
}

/// Returns true if the specified record refers to Python.
fn is_python_record(record: &PackageRecord) -> bool {
    record.name.as_normalized() == "python"
}
