use std::{ffi::{OsStr, OsString}, path::{Path, PathBuf}, sync::Arc};

use rattler_conda_types::{package::{PathType, PrefixPlaceholder}};


#[derive(Debug)]
pub struct FuseDirectory {
    pub prefix_path: PathBuf,
    pub parent: usize,
    pub children: Vec<usize>
}
impl FuseDirectory{
    fn new(prefix_path: PathBuf, parent: usize) -> Self {
        FuseDirectory{
                prefix_path,
                parent: parent,
                children: vec![]
            }
    }
}

#[derive(Debug)]
pub struct FuseFile {
    pub file_name: OsString,
    pub parent: usize,
    pub cache_base_path: Arc<Path>,
    pub _path_type: PathType,
    pub prefix_placeholder: Option<PrefixPlaceholder> 
}

impl FuseFile {
    fn new(file_name: OsString, parent: usize, cache_base_path: Arc<Path>, _path_type: PathType, prefix_placeholder: Option<PrefixPlaceholder>) -> Self {
        FuseFile{
            file_name,
            parent,
            cache_base_path,
            _path_type,
            prefix_placeholder
        }
    }
}

#[derive(Debug)]
pub enum FuseMetadata {
    Directory(FuseDirectory),
    File(FuseFile)
}

impl FuseMetadata {
    pub fn file_name(&self) -> &OsStr {
        match self {
            Self::Directory(directory) => directory.prefix_path.file_name().unwrap(),
            Self::File(file) => &file.file_name
        }
    }
    pub fn new_directory (prefix_path: PathBuf, parent: usize) -> Self {
        FuseMetadata::Directory(FuseDirectory::new(prefix_path, parent))
    }
    pub fn new_file(file_name: OsString, parent: usize, cache_base_path: Arc<Path>, path_type: PathType, prefix_placeholder: Option<PrefixPlaceholder> ) -> Self {
        FuseMetadata::File(FuseFile::new(file_name, parent, cache_base_path, path_type, prefix_placeholder))
    }
    pub fn as_directory(&self) -> Option<&FuseDirectory> {
        match self {
            Self::Directory(directory) => Some(directory),
            Self::File(_) => None
        }
    }
    pub fn as_directory_mut(&mut self) -> Option<&mut FuseDirectory> {
        match self {
            Self::Directory(directory) => Some(directory),
            Self::File(_) => None
        }
    }
    pub fn as_file(&self) -> Option<&FuseFile>{
        match self {
            Self::File(file) => Some(file),
            Self::Directory(_) => None
        }
    }
    
    pub fn as_file_mut(&mut self)  -> Option<&mut FuseFile>{
        match self {
            Self::File(file) => Some(file),
            Self::Directory(_) => None
        }
    }
}