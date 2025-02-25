use known_folders::{get_known_folder_path, KnownFolder};
use std::collections::HashMap;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FolderError {
    #[error("Path not found")]
    PathNotFound,
}

pub struct Folders {
    system_folders: HashMap<Folder, KnownFolder>,
    user_folders: HashMap<Folder, KnownFolder>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UserHandle {
    Current,
    Common,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Folder {
    Desktop,
    Start,
    Documents,
    Profile,
    QuickLaunch,
    LocalAppData,
}

impl Folders {
    pub fn new() -> Self {
        let mut system_folders = HashMap::new();
        system_folders.insert(Folder::Desktop, KnownFolder::PublicDesktop);
        system_folders.insert(Folder::Start, KnownFolder::CommonPrograms);
        system_folders.insert(Folder::Documents, KnownFolder::PublicDocuments);
        system_folders.insert(Folder::Profile, KnownFolder::ProgramData);

        let mut user_folders = HashMap::new();
        user_folders.insert(Folder::Desktop, KnownFolder::Desktop);
        user_folders.insert(Folder::Start, KnownFolder::Programs);
        user_folders.insert(Folder::Documents, KnownFolder::Documents);
        user_folders.insert(Folder::Profile, KnownFolder::Profile);
        user_folders.insert(Folder::QuickLaunch, KnownFolder::QuickLaunch);
        user_folders.insert(Folder::LocalAppData, KnownFolder::LocalAppData);

        Folders {
            system_folders,
            user_folders,
        }
    }

    pub fn get_folder_path(
        &self,
        key: Folder,
        user_handle: UserHandle,
    ) -> Result<PathBuf, FolderError> {
        self.folder_path(user_handle, true, key)
    }

    fn folder_path(
        &self,
        preferred_mode: UserHandle,
        check_other_mode: bool,
        key: Folder,
    ) -> Result<PathBuf, FolderError> {
        let (preferred_folders, other_folders) = match preferred_mode {
            UserHandle::Current => (&self.user_folders, &self.system_folders),
            UserHandle::Common => (&self.system_folders, &self.user_folders),
        };

        if let Some(folder) = preferred_folders.get(&key) {
            if let Some(path) = get_known_folder_path(*folder) {
                return Ok(path);
            }
        }

        // Implement fallback for user documents
        if preferred_mode == UserHandle::Current && key == Folder::Documents {
            if let Some(profile_folder) = preferred_folders.get(&Folder::Profile) {
                if let Some(profile_path) = get_known_folder_path(*profile_folder) {
                    let documents_path = profile_path.join("Documents");
                    if documents_path.is_dir() {
                        return Ok(documents_path);
                    }
                }
            }
        }

        if check_other_mode {
            if let Some(folder) = other_folders.get(&key) {
                if let Some(path) = get_known_folder_path(*folder) {
                    return Ok(path);
                }
            }
        }

        Err(FolderError::PathNotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_folder_path() {
        let folders = Folders::new();

        let test_folders = vec![
            (Folder::Desktop, UserHandle::Current),
            (Folder::Documents, UserHandle::Current),
            (Folder::Start, UserHandle::Common),
            (Folder::Profile, UserHandle::Common),
        ];

        for (folder, handle) in test_folders {
            match folders.get_folder_path(folder, handle) {
                Ok(path) => {
                    println!("{folder:?} path for {handle:?}: {path:?}");
                    assert!(path.exists())
                }
                Err(e) => println!("Error getting {folder:?} path for {handle:?}: {e:?}"),
            }
        }
    }
}
