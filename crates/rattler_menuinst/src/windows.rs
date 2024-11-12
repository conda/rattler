use std::path::Path;

use crate::{
    schema::{MenuItemCommand, Windows},
    MenuInstError, MenuMode,
};

// mod knownfolders;
// mod registry;
//
// struct Directories {
//     start_menu_location: PathBuf,
//     quick_launch_location: PathBuf,
//     desktop_location: PathBuf,
// }
//
// impl Directories {
//     pub fn create() -> Directories {
//         let start_menu_location = dirs::start_menu_dir().unwrap();
//         let quick_launch_location = dirs::quick_launch_dir().unwrap();
//         let desktop_location = dirs::desktop_dir().unwrap();
//
//         Directories {
//             start_menu_location,
//             quick_launch_location,
//             desktop_location,
//         }
//     }
// }
//
// pub struct WindowsMenu {
//     name: String,
//     mode: String,
//     prefix: PathBuf,
//     base_prefix: PathBuf,
// }

pub(crate) fn install_menu_item(
    prefix: &Path,
    windows_item: Windows,
    command: MenuItemCommand,
    menu_mode: MenuMode,
) -> Result<(), MenuInstError> {
    // let bundle_name = macos_item.cf_bundle_name.as_ref().unwrap();
    // let directories = Directories::new(menu_mode, bundle_name);
    // println!("Installing menu item for {bundle_name}");
    // let menu = crate::macos::MacOSMenu::new(prefix, macos_item, command,
    // directories); menu.install()
    Ok(())
}
