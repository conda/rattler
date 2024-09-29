use std::path::PathBuf;

mod knownfolders;
mod registry;

struct Directories {
    start_menu_location: PathBuf,
    quick_launch_location: PathBuf,
    desktop_location: PathBuf,
}

impl Directories {
    pub fn create() -> Directories {
        let start_menu_location =
            known_folders::get_known_folder_path(known_folders::KnownFolder::StartMenu)
                .expect("Failed to get start menu location");
        let quick_launch_location =
            known_folders::get_known_folder_path(known_folders::KnownFolder::QuickLaunch)
                .expect("Failed to get quick launch location");
        let desktop_location =
            known_folders::get_known_folder_path(known_folders::KnownFolder::Desktop)
                .expect("Failed to get desktop location");

        Directories {
            start_menu_location,
            quick_launch_location,
            desktop_location,
        }
    }
}

pub struct WindowsMenu {
    name: String,
    mode: String,
    prefix: PathBuf,
    base_prefix: PathBuf,
}
