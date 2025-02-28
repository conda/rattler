use std::path::{Path, PathBuf};

use rattler_conda_types::{
    menuinst::{MenuMode, Tracker},
    Platform, PrefixRecord,
};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod render;
pub mod schema;
#[cfg(target_os = "windows")]
mod windows;

use crate::{render::BaseMenuItemPlaceholders, schema::MenuInstSchema};

mod utils;

#[derive(thiserror::Error, Debug)]
pub enum MenuInstError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("deserialization error: {0}")]
    SerdeError(#[from] serde_json::Error),

    #[error("failed to install menu item: {0}")]
    InstallError(String),

    #[error("invalid path: {0}")]
    InvalidPath(PathBuf),

    #[cfg(target_os = "linux")]
    #[error("could not quote command with shlex: {0}")]
    ShlexQuoteError(#[from] shlex::QuoteError),

    #[cfg(target_os = "macos")]
    #[error("failed to create plist: {0}")]
    PlistError(#[from] plist::Error),

    #[cfg(target_os = "macos")]
    #[error("failed to sign plist: {0}")]
    SigningFailed(String),

    #[error("failed to install menu item: {0}")]
    ActivationError(#[from] rattler_shell::activation::ActivationError),

    #[cfg(target_os = "linux")]
    #[error("failed to install menu item: {0}")]
    XmlError(#[from] quick_xml::Error),

    #[cfg(target_os = "windows")]
    #[error("failed to install menu item: {0}")]
    WindowsError(#[from] ::windows::core::Error),

    #[cfg(target_os = "windows")]
    #[error("failed to register terminal profile: {0}")]
    TerminalProfileError(#[from] windows::TerminalUpdateError),

    #[cfg(target_os = "linux")]
    #[error("menu config location is not a file: {0:?}")]
    MenuConfigNotAFile(PathBuf),
}

/// Install menu items for a given prefix record according to `Menu/*.json` files
/// Note: this function will update the prefix record with the installed menu items
/// and write it back to the prefix record file if any Menu item is found
pub fn install_menuitems_for_record(
    target_prefix: &Path,
    prefix_record: &PrefixRecord,
    platform: Platform,
    menu_mode: MenuMode,
) -> Result<(), MenuInstError> {
    // Look for Menu/*.json files in the package paths
    let menu_files: Vec<_> = prefix_record
        .paths_data
        .paths
        .iter()
        .filter(|path| {
            path.relative_path.starts_with("Menu/")
                && path
                    .relative_path
                    .extension()
                    .is_some_and(|ext| ext == "json")
        })
        .collect();

    for menu_file in menu_files {
        let full_path = target_prefix.join(&menu_file.relative_path);
        let tracker_vec = install_menuitems(
            &full_path,
            target_prefix,
            target_prefix,
            platform,
            menu_mode,
        )?;

        // Store tracker in the prefix record
        let mut record = prefix_record.clone();
        record.installed_system_menus = tracker_vec;

        // Save the updated prefix record
        record
            .write_to_path(
                target_prefix.join("conda-meta").join(record.file_name()),
                true,
            )
            .expect("Failed to write prefix record");
    }

    Ok(())
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
                macos::remove_menu_item(tracker)?;
            }
            Tracker::Linux(tracker) => {
                #[cfg(target_os = "linux")]
                linux::remove_menu_item(tracker)?;
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
