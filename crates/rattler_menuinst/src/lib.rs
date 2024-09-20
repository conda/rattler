use std::path::Path;

use rattler_conda_types::Platform;

mod linux;
mod macos;
mod render;
mod schema;

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
        if item.platforms.linux.is_some() && platform.is_linux() {
            let mut linux_item = item.platforms.linux.clone().unwrap();
            let base_item = linux_item.base.merge_parent(&item);
            linux_item.base = base_item;
            linux::install_menu_item(linux_item, MenuMode::System)?;
        } else if item.platforms.osx.is_some() && platform.is_osx() {
            let mut macos_item = item.platforms.osx.clone().unwrap();
            let base_item = macos_item.base.merge_parent(&item);
            macos_item.base = base_item;
            macos::install_menu_item(prefix, macos_item, MenuMode::System)?;
        } else if item.platforms.win.is_some() && platform.is_windows() {
            // windows::install_menu_item(&item)?;
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
