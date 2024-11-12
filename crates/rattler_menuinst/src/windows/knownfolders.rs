use known_folders::{get_known_folder_path, KnownFolder};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum FolderError {
    PathNotFound,
    PathNotVerifiable,
}

pub struct Folders {
    system_folders: HashMap<String, KnownFolder>,
    user_folders: HashMap<String, KnownFolder>,
}

#[derive(Clone, Copy)]
pub enum UserHandle {
    Current,
    Common,
}

impl Folders {
    pub fn new() -> Self {
        let mut system_folders = HashMap::new();
        system_folders.insert("desktop".to_string(), KnownFolder::PublicDesktop);
        system_folders.insert("start".to_string(), KnownFolder::CommonPrograms);
        system_folders.insert("documents".to_string(), KnownFolder::PublicDocuments);
        system_folders.insert("profile".to_string(), KnownFolder::ProgramData);

        let mut user_folders = HashMap::new();
        user_folders.insert("desktop".to_string(), KnownFolder::Desktop);
        user_folders.insert("start".to_string(), KnownFolder::Programs);
        user_folders.insert("documents".to_string(), KnownFolder::Documents);
        user_folders.insert("profile".to_string(), KnownFolder::Profile);

        Folders {
            system_folders,
            user_folders,
        }
    }

    pub fn get_folder_path(
        &self,
        key: &str,
        user_handle: UserHandle,
    ) -> Result<PathBuf, FolderError> {
        self.folder_path(user_handle, true, key)
    }

    fn folder_path(
        &self,
        preferred_mode: UserHandle,
        check_other_mode: bool,
        key: &str,
    ) -> Result<PathBuf, FolderError> {
        let (preferred_folders, other_folders) = match preferred_mode {
            UserHandle::Current => (&self.user_folders, &self.system_folders),
            UserHandle::Common => (&self.system_folders, &self.user_folders),
        };

        if let Some(folder) = preferred_folders.get(key) {
            if let Some(path) = get_known_folder_path(folder) {
                return Ok(path);
            }
        }

        // Implement fallback for user documents
        if preferred_mode == UserHandle::Current && key == "documents" {
            if let Some(profile_folder) = preferred_folders.get("profile") {
                if let Some(profile_path) = get_known_folder_path(profile_folder) {
                    let documents_path = profile_path.join("Documents");
                    if documents_path.is_dir() {
                        return Ok(documents_path);
                    }
                }
            }
        }

        if check_other_mode {
            if let Some(folder) = other_folders.get(key) {
                if let Some(path) = get_known_folder_path(folder) {
                    return Ok(path);
                }
            }
        }

        Err(FolderError::PathNotFound)
    }

    pub fn verify_path<P: AsRef<Path>>(&self, path: P) -> Result<PathBuf, FolderError> {
        let path = path.as_ref();
        if path.exists() && path.is_dir() {
            Ok(path.to_path_buf())
        } else {
            Err(FolderError::PathNotVerifiable)
        }
    }
}

fn main() {
    let folders = Folders::new();

    let test_folders = vec![
        ("desktop", UserHandle::Current),
        ("documents", UserHandle::Current),
        ("start", UserHandle::Common),
        ("profile", UserHandle::Common),
    ];

    for (folder, handle) in test_folders {
        match folders.get_folder_path(folder, handle) {
            Ok(path) => {
                tracing::info!("{} path for {:?}: {:?}", folder, handle, path);
                match folders.verify_path(&path) {
                    Ok(_) => println!("  Path verified successfully"),
                    Err(e) => println!("  Path verification failed: {:?}", e),
                }
            }
            Err(e) => tracing::error!("Error getting {} path for {:?}: {:?}", folder, handle, e),
        }
    }
}
