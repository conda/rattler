use std::path::Path;

use rattler_conda_types::Platform;

mod linux;
mod macos;
mod render;
mod schema;
#[cfg(target_os = "windows")]
mod windows;

pub mod slugify;
pub use slugify::slugify;

pub mod utils;

#[derive(Debug, Eq, PartialEq)]
pub enum MenuMode {
    System,
    User,
}

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
}

// Install menu items from a given schema file
pub fn install_menuitems(
    file: &Path,
    prefix: &Path,
    base_prefix: &Path,
    platform: &Platform,
) -> Result<(), MenuInstError> {
    let placeholders = render::placeholders(base_prefix, prefix, platform);

    let schema = render::render(file, &placeholders)?;

    for item in schema.menu_items {
        if platform.is_linux() {
            if let Some(linux_item) = item.platforms.linux {
                let command = item.command.merge(linux_item.base);
                linux::install_menu_item(linux_item.specific, command, MenuMode::System)?;
            }
        } else if platform.is_osx() {
            if let Some(macos_item) = item.platforms.osx {
                let command = item.command.merge(macos_item.base);
                macos::install_menu_item(prefix, macos_item.specific, command, MenuMode::System)?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
pub mod test {
    use std::path::{Path, PathBuf};

    use rattler_conda_types::Platform;

    use crate::install_menuitems;

    pub(crate) fn test_data() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data/menuinst")
    }

    #[test]
    fn test_install_menu_item() {
        println!("Running test_install_menu_item");
        let test_data = crate::test::test_data();
        let schema_path = test_data.join("mne_menu/menu/menu.json");

        let prefix = schema_path.parent().unwrap().parent().unwrap();
        let prefix = std::fs::canonicalize(prefix).unwrap();
        println!("prefix: {:?}", prefix);
        let base_prefix = PathBuf::from("/Users/jaidevd/miniconda3");
        let platform = Platform::OsxArm64;

        install_menuitems(&schema_path, &prefix, &base_prefix, &platform).unwrap();
    }
}
