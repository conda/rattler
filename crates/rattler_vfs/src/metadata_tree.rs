//! In-memory metadata tree shared by all transports.
//!
//! Each [`MetadataNode`] is either a [`DirectoryNode`] (children indices into
//! the same `Vec<MetadataNode>`) or a [`FileNode`] (a leaf with cache path,
//! prefix-replacement metadata, and optional materialized content). The tree
//! is built once by `build_metadata_tree` and consumed by `VirtualFS::new`,
//! after which it backs FUSE, NFS, and `ProjFS` reads alike.
//!
//! Despite the historical `fuse_directory.rs` filename, this is **not**
//! FUSE-specific.

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    sync::Arc,
};

use rattler_conda_types::package::{PathType, PrefixPlaceholder};

#[derive(Debug)]
pub struct DirectoryNode {
    pub prefix_path: PathBuf,
    pub parent: usize,
    pub children: Vec<usize>,
}

impl DirectoryNode {
    fn new(prefix_path: PathBuf, parent: usize) -> Self {
        DirectoryNode {
            prefix_path,
            parent,
            children: vec![],
        }
    }
}

#[derive(Debug)]
pub struct FileNode {
    pub file_name: OsString,
    pub parent: usize,
    pub cache_base_path: Arc<Path>,
    pub path_type: PathType,
    pub prefix_placeholder: Option<PrefixPlaceholder>,
    /// Pre-materialized content for virtual files (e.g. generated entry point scripts).
    /// When set, the FUSE layer serves this content directly instead of reading from disk.
    pub virtual_content: Option<Vec<u8>>,
    /// Pre-computed file size after prefix replacement (for text-mode files).
    /// When set, `_getattr()` uses this instead of the on-disk file size.
    pub computed_size: Option<u64>,
    /// Override for the cache directory path used by `_getpath()`.
    /// When set (e.g. for noarch Python files where the virtual path differs from
    /// the on-disk cache path), `_getpath()` uses this instead of `parent.prefix_path`.
    pub cache_prefix_path: Option<PathBuf>,
}

impl FileNode {
    fn new(
        file_name: OsString,
        parent: usize,
        cache_base_path: Arc<Path>,
        path_type: PathType,
        prefix_placeholder: Option<PrefixPlaceholder>,
    ) -> Self {
        FileNode {
            file_name,
            parent,
            cache_base_path,
            path_type,
            prefix_placeholder,
            virtual_content: None,
            computed_size: None,
            cache_prefix_path: None,
        }
    }

    fn new_virtual(file_name: OsString, parent: usize, content: Vec<u8>) -> Self {
        FileNode {
            file_name,
            parent,
            cache_base_path: Arc::from(Path::new("")),
            path_type: PathType::HardLink,
            prefix_placeholder: None,
            virtual_content: Some(content),
            computed_size: None,
            cache_prefix_path: None,
        }
    }
}

#[derive(Debug)]
pub enum MetadataNode {
    Directory(DirectoryNode),
    File(FileNode),
}

impl MetadataNode {
    pub fn file_name(&self) -> &OsStr {
        match self {
            Self::Directory(directory) => directory
                .prefix_path
                .file_name()
                .unwrap_or(std::ffi::OsStr::new(".")),
            Self::File(file) => &file.file_name,
        }
    }

    pub fn new_directory(prefix_path: PathBuf, parent: usize) -> Self {
        MetadataNode::Directory(DirectoryNode::new(prefix_path, parent))
    }

    pub fn new_file(
        file_name: OsString,
        parent: usize,
        cache_base_path: Arc<Path>,
        path_type: PathType,
        prefix_placeholder: Option<PrefixPlaceholder>,
    ) -> Self {
        MetadataNode::File(FileNode::new(
            file_name,
            parent,
            cache_base_path,
            path_type,
            prefix_placeholder,
        ))
    }

    pub fn new_virtual_file(file_name: OsString, parent: usize, content: Vec<u8>) -> Self {
        MetadataNode::File(FileNode::new_virtual(file_name, parent, content))
    }

    pub fn as_directory(&self) -> Option<&DirectoryNode> {
        if let Self::Directory(directory) = self {
            Some(directory)
        } else {
            None
        }
    }

    pub fn as_directory_mut(&mut self) -> Option<&mut DirectoryNode> {
        if let Self::Directory(directory) = self {
            Some(directory)
        } else {
            None
        }
    }

    pub fn as_file(&self) -> Option<&FileNode> {
        if let Self::File(file) = self {
            Some(file)
        } else {
            None
        }
    }

    pub fn as_file_mut(&mut self) -> Option<&mut FileNode> {
        if let Self::File(file) = self {
            Some(file)
        } else {
            None
        }
    }
}
