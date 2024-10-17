use std::path::{Path, PathBuf};

use crate::{schema, MenuInstError, MenuMode};
use fs_err as fs;
mod knownfolders;
mod registry;
mod create_shortcut;

struct Directories {
    start_menu_location: PathBuf,
    quick_launch_location: Option<PathBuf>,
    desktop_location: Option<PathBuf>,
}

impl Directories {
    pub fn create() -> Directories {
        let start_menu_location =
            known_folders::get_known_folder_path(known_folders::KnownFolder::StartMenu)
                .expect("Failed to get start menu location");
        let quick_launch_location =
            known_folders::get_known_folder_path(known_folders::KnownFolder::QuickLaunch);
        let desktop_location =
            known_folders::get_known_folder_path(known_folders::KnownFolder::Desktop);

        Directories {
            start_menu_location,
            quick_launch_location,
            desktop_location,
        }
    }
}

pub struct WindowsMenu {
    directories: Directories,
    item: schema::Windows,
    mode: MenuMode,
    prefix: PathBuf,
}

impl WindowsMenu {
    pub fn new(item: schema::Windows, prefix: &Path, mode: MenuMode) -> WindowsMenu {
        let directories = Directories::create();

        WindowsMenu {
            directories,
            item,
            mode,
            prefix: prefix.to_path_buf(),
        }
    }

    pub fn create(&self) -> Result<PathBuf, MenuInstError> {
        tracing::debug!("Creating {:?}", self.directories.start_menu_location);
        fs::create_dir_all(&self.directories.start_menu_location)?;

        if let Some(ref quick_launch_location) = self.directories.quick_launch_location {
            fs::create_dir_all(quick_launch_location)?;
        }

        if let Some(ref desktop_location) = self.directories.desktop_location {
            fs::create_dir_all(desktop_location)?;
        }

        Ok(self.directories.start_menu_location.clone())
    }

    pub fn remove(&self) -> Result<PathBuf, MenuInstError> {
        let menu_location = &self.directories.start_menu_location;
        if menu_location.exists() {
            if menu_location.read_dir()?.next().is_none() {
                tracing::info!("Removing {menu_location:?}", );
                fs::remove_dir_all(menu_location)?;
            }
        }
        Ok(self.directories.start_menu_location.clone())
    }
}

struct WindowsMenuItem {
    item: schema::Windows,
    prefix: PathBuf,
}


