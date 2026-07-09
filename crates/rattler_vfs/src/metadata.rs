use memchr::memmem;
use memmap2::Mmap;
use rattler_conda_types::package::{FileMode, PathType, PrefixPlaceholder};
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CustomPrefixPlaceholder {
    pub file_mode: FileMode,
    /// The build-time prefix path that appears verbatim in the cached file.
    pub placeholder: String,
    /// Byte offsets where `placeholder` starts in the file.
    pub offsets: Vec<usize>,
}

impl CustomPrefixPlaceholder {
    pub fn from_placeholder(placeholder: PrefixPlaceholder, source_bytes: &[u8]) -> Self {
        let offsets = memmem::find_iter(source_bytes, placeholder.placeholder.as_bytes()).collect();
        CustomPrefixPlaceholder {
            file_mode: placeholder.file_mode,
            placeholder: placeholder.placeholder,
            offsets,
        }
    }

    pub fn fill_offsets(&mut self, open_file: &Mmap) {
        let mut offsets = memmem::find_iter(open_file, self.placeholder.as_bytes()).collect();
        self.offsets.append(&mut offsets);
    }
}

#[derive(Debug)]
pub struct FSDirectory {
    pub prefix_path: PathBuf,
    pub parent: usize,
    pub children: Vec<usize>,
}

impl FSDirectory {
    fn new(prefix_path: PathBuf, parent: usize) -> Self {
        FSDirectory {
            prefix_path,
            parent,
            children: vec![],
        }
    }
}

#[derive(Debug)]
pub struct FSFile {
    pub file_name: OsString,
    pub parent: usize,
    pub cache_base_path: Arc<Path>,
    pub path_type: PathType,
    pub prefix_placeholder: Option<CustomPrefixPlaceholder>,
}

impl FSFile {
    fn new(
        file_name: OsString,
        parent: usize,
        cache_base_path: Arc<Path>,
        path_type: PathType,
        prefix_placeholder: Option<CustomPrefixPlaceholder>,
    ) -> Self {
        FSFile {
            file_name,
            parent,
            cache_base_path,
            path_type,
            prefix_placeholder,
        }
    }
}

#[derive(Debug)]
pub enum FSMetadata {
    FSDirectory(FSDirectory),
    FSFile(FSFile),
}

impl FSMetadata {
    pub fn file_name(&self) -> &OsStr {
        match self {
            Self::FSDirectory(directory) => directory
                .prefix_path
                .file_name()
                .unwrap_or_else(|| OsStr::new(".")),
            Self::FSFile(file) => &file.file_name,
        }
    }

    pub fn new_directory(prefix_path: PathBuf, parent: usize) -> Self {
        FSMetadata::FSDirectory(FSDirectory::new(prefix_path, parent))
    }

    pub fn new_file(
        file_name: OsString,
        parent: usize,
        cache_base_path: Arc<Path>,
        path_type: PathType,
        prefix_placeholder: Option<CustomPrefixPlaceholder>,
    ) -> Self {
        FSMetadata::FSFile(FSFile::new(
            file_name,
            parent,
            cache_base_path,
            path_type,
            prefix_placeholder,
        ))
    }

    pub fn as_directory(&self) -> Option<&FSDirectory> {
        match self {
            Self::FSDirectory(directory) => Some(directory),
            Self::FSFile(_) => None,
        }
    }

    pub fn as_directory_mut(&mut self) -> Option<&mut FSDirectory> {
        match self {
            Self::FSDirectory(directory) => Some(directory),
            Self::FSFile(_) => None,
        }
    }

    pub fn as_file(&self) -> Option<&FSFile> {
        match self {
            Self::FSFile(file) => Some(file),
            Self::FSDirectory(_) => None,
        }
    }

    pub fn as_file_mut(&mut self) -> Option<&mut FSFile> {
        match self {
            Self::FSFile(file) => Some(file),
            Self::FSDirectory(_) => None,
        }
    }
}
