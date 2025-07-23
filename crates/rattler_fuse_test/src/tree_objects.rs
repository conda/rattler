use crate::patching::{OpenFile, PatchedFile};
use fuser::{FileAttr, FileType};
use rattler::install::python_entry_point_template;
use rattler::install::PythonInfo;
use rattler_conda_types::package;
use rattler_conda_types::Platform;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::os::linux::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::UNIX_EPOCH;

/// Error returned when attempting to treat a non-directory as a directory
#[derive(Debug)]
pub struct NotADirectoryError;

pub type NodeRef = Rc<RefCell<Node>>;
pub type NodeWeak = Weak<RefCell<Node>>;

impl fmt::Display for NotADirectoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a directory")
    }
}

impl std::error::Error for NotADirectoryError {}

#[derive(Debug)]
pub enum PatchMode {
    /// The file does not need any patching
    None,
    /// The file is a binary file (needs binary prefix replacement)
    Binary(Vec<u8>, Rc<Vec<u8>>, Platform),
    /// The file is a text file (needs text prefix replacement)
    Text(Vec<u8>, Rc<Vec<u8>>, Platform),
}

impl Clone for PatchMode {
    fn clone(&self) -> Self {
        match self {
            PatchMode::None => PatchMode::None,
            PatchMode::Binary(bytes, target_path, target_platform) => {
                PatchMode::Binary(bytes.clone(), target_path.clone(), *target_platform)
            }
            PatchMode::Text(text, target_path, target_platform) => {
                PatchMode::Text(text.clone(), target_path.clone(), *target_platform)
            }
        }
    }
}

/// Represents the three kinds of nodes in the filesystem tree
pub enum Node {
    File(File),
    Directory(Directory),
    Symlink(Symlink),
    EntryPoint(EntryPoint),
}

/// Represents a File node
#[derive(Debug)]
pub struct File {
    ino: u64,
    name: OsString,
    parent: NodeWeak,
    // File specific attributes
    target: PathBuf,
    size: u64,
    patch_mode: PatchMode,
}

/// Represents a Directory node
pub struct Directory {
    ino: u64,
    name: OsString,
    parent: NodeWeak,
    // Directory specific attributes
    children: HashMap<OsString, NodeRef>,
}

/// Represents a Symlink node
pub struct Symlink {
    ino: u64,
    name: OsString,
    parent: NodeWeak,
    // Symlink specific attributes
    target: PathBuf,
}

/// Represents an entry point node
pub struct EntryPoint {
    ino: u64,
    name: OsString,
    parent: NodeWeak,
    // EntryPoint specific attributes
    target_prefix: Rc<Vec<u8>>,
    python_info: Rc<PythonInfo>,
    module: String,
    function: String,
}

impl Node {
    pub fn name(&self) -> &OsStr {
        match self {
            Node::File(file) => &file.name,
            Node::Directory(dir) => &dir.name,
            Node::Symlink(symlink) => &symlink.name,
            Node::EntryPoint(entry_point) => &entry_point.name,
        }
    }

    pub fn ino(&self) -> u64 {
        match self {
            Node::File(file) => file.ino,
            Node::Directory(dir) => dir.ino,
            Node::Symlink(symlink) => symlink.ino,
            Node::EntryPoint(entry_point) => entry_point.ino,
        }
    }

    pub fn parent(&self) -> NodeRef {
        let parent = match self {
            Node::Directory(dir) => &dir.parent,
            Node::File(file) => &file.parent,
            Node::Symlink(symlink) => &symlink.parent,
            Node::EntryPoint(entry_point) => &entry_point.parent,
        };
        parent.upgrade().expect("Parent directory not found")
    }

    pub fn print_tree(&self, depth: usize) {
        let indent = "  ".repeat(depth);
        match self {
            Node::File(file) => println!(
                "{}📄 {:?} ({}) -> ({:?})",
                indent, file.name, file.size, file.target
            ),
            Node::EntryPoint(ep) => println!(
                "{}🐍 {:?} -> ({}.{}())",
                indent, ep.name, ep.module, ep.function
            ),
            Node::Directory(dir) => {
                println!("{}📂 {:?}", indent, dir.name);
                for child in dir.children.values() {
                    child.borrow().print_tree(depth + 1);
                }
            }
            Node::Symlink(symlink) => {
                println!("{}🔗 {:?} -> {:?}", indent, symlink.name, symlink.target);
            }
        }
    }

    pub fn as_directory(&self) -> Result<&Directory, NotADirectoryError> {
        match self {
            Node::Directory(dir) => Ok(dir),
            _ => Err(NotADirectoryError),
        }
    }

    pub fn as_symlink(&self) -> Result<&Symlink, NotADirectoryError> {
        match self {
            Node::Symlink(symlink) => Ok(symlink),
            _ => Err(NotADirectoryError),
        }
    }

    pub fn stat(&self, uid: u32, gid: u32) -> FileAttr {
        match self {
            Node::Directory(dir) => dir.stat(uid, gid),
            Node::File(file) => file.stat(uid, gid),
            Node::Symlink(symlink) => symlink.stat(uid, gid),
            Node::EntryPoint(entrypoint) => entrypoint.stat(uid, gid),
        }
    }

    pub fn open(&self) -> OpenFile {
        match self {
            Node::File(file) => file.open(),
            Node::EntryPoint(entry_point) => {
                OpenFile::InMemory(entry_point.script().into_bytes())
            }
            _ => panic!("Cannot open non-file node"),
        }
    }
}

impl Directory {
    pub fn new(ino: u64, name: OsString, parent: Option<NodeWeak>) -> Directory {
        let parent = match parent {
            Some(parent) => parent,
            None => Weak::new(),
        };
        Directory {
            ino,
            name,
            parent,
            children: HashMap::new(),
        }
    }

    pub fn add_child(&mut self, child: NodeRef) {
        let child_name = {
            let child = child.borrow();
            child.name().to_os_string()
        };
        self.children.insert(child_name, child);
    }

    pub fn get_child(&self, name: &OsStr) -> Option<NodeRef> {
        self.children.get(name).cloned()
    }

    pub fn children(&self) -> Vec<NodeRef> {
        self.children.values().cloned().collect()
    }

    fn stat(&self, uid: u32, gid: u32) -> FileAttr {
        FileAttr {
            ino: self.ino,
            size: 0,
            blocks: 0,
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: FileType::Directory,
            perm: 0o755,
            nlink: 1,
            uid,
            gid,
            rdev: 0,
            flags: 0,
            blksize: 512,
        }
    }
}

impl File {
    pub fn new(
        ino: u64,
        name: OsString,
        parent: NodeWeak,
        target: PathBuf,
        size: u64,
        patch_mode: PatchMode,
    ) -> File {
        File {
            ino,
            name,
            parent,
            target,
            size,
            patch_mode,
        }
    }

    pub fn open(&self) -> OpenFile {
        let f = std::fs::File::open(&self.target).unwrap();
        PatchedFile::open(f, &self.patch_mode)
    }

    fn stat(&self, uid: u32, gid: u32) -> FileAttr {
        let metadata =
            std::fs::symlink_metadata(&self.target).expect("Failed to get metadata for file");
        let kind = if metadata.file_type().is_symlink() {
            FileType::Symlink
        } else {
            FileType::RegularFile
        };
        let size = match &self.patch_mode {
            PatchMode::None | PatchMode::Binary(_, _, _) => self.size,
            PatchMode::Text(_, _, _) => {
                let mut f = self.open();
                (self.size as i64 + f.size_change()) as u64
            }
        };
        FileAttr {
            ino: self.ino,
            size,
            blocks: metadata.st_blocks(),
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind,
            perm: metadata.permissions().mode() as u16,
            nlink: 1,
            uid,
            gid,
            rdev: 0,
            flags: 0,
            blksize: metadata.st_blksize() as u32,
        }
    }
}

impl Symlink {
    pub fn new(ino: u64, name: OsString, parent: NodeWeak, target: PathBuf) -> Symlink {
        Symlink {
            ino,
            name,
            parent,
            target,
        }
    }

    pub fn readlink(&self) -> PathBuf {
        std::fs::read_link(&self.target).expect("Failed to read symlink target")
    }

    fn stat(&self, uid: u32, gid: u32) -> FileAttr {
        let metadata =
            std::fs::symlink_metadata(&self.target).expect("Failed to get metadata for symlink");
        FileAttr {
            ino: self.ino,
            size: metadata.len(),
            blocks: metadata.st_blocks(),
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: FileType::Symlink,
            perm: metadata.permissions().mode() as u16,
            nlink: 1,
            uid,
            gid,
            rdev: 0,
            flags: 0,
            blksize: metadata.st_blksize() as u32,
        }
    }
}

impl EntryPoint {
    pub fn new(
        ino: u64,
        name: OsString,
        parent: NodeWeak,
        module: String,
        function: String,
        target_prefix: Rc<Vec<u8>>,
        python_info: Rc<PythonInfo>,
    ) -> EntryPoint {
        EntryPoint {
            ino,
            name,
            parent,
            target_prefix: Rc::clone(&target_prefix),
            python_info: Rc::clone(&python_info),
            module,
            function,
        }
    }

    fn script(&self) -> String {
        let entry_point = package::EntryPoint {
            command: self
                .name
                .clone()
                .into_string()
                .expect("Name should be valid UTF-8"),
            module: self.module.clone(),
            function: self.function.clone(),
        };
        python_entry_point_template(
            std::str::from_utf8(self.target_prefix.as_slice())
                .expect("Invalid UTF-8 in target_prefix"),
            false,
            &entry_point,
            &self.python_info,
        )
    }

    fn stat(&self, uid: u32, gid: u32) -> FileAttr {
        FileAttr {
            ino: self.ino,
            size: self.script().len() as u64,
            blocks: (self.script().len() as u64) / 512 + 1,
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: FileType::RegularFile,
            perm: 0o755,
            nlink: 1,
            uid,
            gid,
            rdev: 0,
            flags: 0,
            blksize: 512,
        }
    }
}
