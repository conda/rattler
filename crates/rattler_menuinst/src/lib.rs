use std::path::Path;

#[cfg(target_os = "linux")]
use std::path::PathBuf;

use rattler_conda_types::{
    menuinst::{MenuMode, Tracker},
    Platform,
};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod render;
mod schema;
mod util;
#[cfg(target_os = "windows")]
mod windows;

pub mod slugify;
pub use slugify::slugify;

use crate::{render::BaseMenuItemPlaceholders, schema::MenuInstSchema};

pub mod utils;

#[derive(thiserror::Error, Debug)]
pub enum MenuInstError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Deserialization error: {0}")]
    SerdeError(#[from] serde_json::Error),

    #[error("Failed to install menu item: {0}")]
    InstallError(String),

    #[error("Failed to create plist: {0}")]
    PlistError(#[from] plist::Error),
    #[error("Failed to sign plist: {0}")]
    SigningFailed(String),

    #[error("Failed to install menu item: {0}")]
    ActivationError(#[from] rattler_shell::activation::ActivationError),

    #[cfg(target_os = "linux")]
    #[error("Failed to install menu item: {0}")]
    XmlError(#[from] quick_xml::Error),

    #[cfg(target_os = "windows")]
    #[error("Failed to install menu item: {0}")]
    WindowsError(#[from] ::windows::core::Error),

    #[cfg(target_os = "linux")]
    #[error("Menu config location is not a file: {0:?}")]
    MenuConfigNotAFile(PathBuf),
}

// Install menu items from a given schema file
pub fn install_menuitems(
    file: &Path,
    prefix: &Path,
    base_prefix: &Path,
    platform: Platform,
    menu_mode: MenuMode,
) -> Result<Vec<Tracker>, MenuInstError> {
    let text = std::fs::read_to_string(file)?;
    let menu_inst: MenuInstSchema = serde_json::from_str(&text)?;
    let placeholders = BaseMenuItemPlaceholders::new(base_prefix, prefix, platform);

    let mut trackers = Vec::new();
    for item in menu_inst.menu_items {
        if platform.is_linux() {
            #[cfg(target_os = "linux")]
            if let Some(linux_item) = item.platforms.linux {
                let command = item.command.merge(linux_item.base);
                let linux_tracker = linux::install_menu_item(
                    &menu_inst.menu_name,
                    prefix,
                    linux_item.specific,
                    command,
                    &placeholders,
                    menu_mode,
                )?;
                trackers.push(Tracker::Linux(linux_tracker));
            }
        } else if platform.is_osx() {
            #[cfg(target_os = "macos")]
            if let Some(macos_item) = item.platforms.osx {
                let command = item.command.merge(macos_item.base);
                let macos_tracker = macos::install_menu_item(
                    prefix,
                    macos_item.specific,
                    command,
                    &placeholders,
                    menu_mode,
                )?;
                trackers.push(Tracker::MacOs(macos_tracker));
            };
        } else if platform.is_windows() {
            #[cfg(target_os = "windows")]
            if let Some(windows_item) = item.platforms.win {
                let command = item.command.merge(windows_item.base);
                let tracker = windows::install_menu_item(
                    prefix,
                    windows_item.specific,
                    command,
                    &placeholders,
                    menu_mode,
                )?;
                trackers.push(Tracker::Windows(tracker));
            }
        }
    }

    Ok(trackers)
}

/// Remove menu items from a given schema file
pub fn remove_menu_items(tracker: &Vec<Tracker>) -> Result<(), MenuInstError> {
    for el in tracker {
        #[allow(unused)]
        match el {
            Tracker::MacOs(tracker) => {
                #[cfg(target_os = "macos")]
                macos::remove_menu_item(tracker).unwrap();
            }
            Tracker::Linux(tracker) => {
                #[cfg(target_os = "linux")]
                linux::remove_menu_item(tracker).unwrap();
            }
            Tracker::Windows(tracker) => {
                #[cfg(target_os = "windows")]
                windows::remove_menu_item(tracker)?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
pub mod test {
    use std::path::{Path, PathBuf};

    #[allow(dead_code)]
    pub(crate) fn test_data() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data")
    }
}
