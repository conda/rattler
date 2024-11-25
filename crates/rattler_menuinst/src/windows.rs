use std::path::{Path, PathBuf};

use knownfolders::UserHandle;

use crate::{
    render::{BaseMenuItemPlaceholders, MenuItemPlaceholders},
    schema::{Environment, MenuItemCommand, Windows},
    MenuInstError, MenuMode,
};

mod knownfolders;
// mod registry;
//
pub struct Directories {
    start_menu: PathBuf,
    quick_launch: PathBuf,
    desktop: PathBuf,
    programs: PathBuf,
}

impl Directories {
    pub fn create() -> Directories {
        let known_folders = knownfolders::Folders::new();
        let start_menu = known_folders
            .get_folder_path("start", UserHandle::Current)
            .unwrap();
        let quick_launch = known_folders
            .get_folder_path("quick_launch", UserHandle::Current)
            .unwrap();
        let desktop = known_folders
            .get_folder_path("desktop", UserHandle::Current)
            .unwrap();
        let programs = known_folders
            .get_folder_path("programs", UserHandle::Current)
            .unwrap();

        Directories {
            start_menu,
            quick_launch,
            desktop,
            programs,
        }
    }
}

pub struct WindowsMenu {
    prefix: PathBuf,
    name: String,
    item: Windows,
    command: MenuItemCommand,
    directories: Directories,
    placeholders: MenuItemPlaceholders,
}

const SHORTCUT_EXTENSION: &str = "lnk";

impl WindowsMenu {
    pub fn new(
        prefix: &Path,
        item: Windows,
        command: MenuItemCommand,
        directories: Directories,
        placeholders: &BaseMenuItemPlaceholders,
    ) -> Self {
        let name = command.name.resolve(Environment::Base, placeholders);

        let programs_link_location = directories
            .programs
            .join(&name)
            .with_extension(SHORTCUT_EXTENSION);

        Self {
            prefix: prefix.to_path_buf(),
            name,
            item,
            command,
            directories,
            placeholders: placeholders.refine(&programs_link_location),
        }
    }

    fn build_command(&self, with_arg1: bool) {
        // TODO handle activation
        let mut command = Vec::new();
        for elem in self.command.command.iter() {
            command.push(elem.resolve(&self.placeholders));
        }

        if with_arg1 && !command.iter().any(|s| s.contains("%1")) {
            command.push("%1".to_string());
        }


    }

    fn create_shortcut(&self, link_path: &Path, args: &str) -> Result<(), MenuInstError> {
        
        // target_path, *arguments = self._process_command()
        // working_dir = self.render_key("working_dir")
        // if working_dir:
        //     Path(working_dir).mkdir(parents=True, exist_ok=True)
        // else:
        //     working_dir = "%HOMEPATH%"

        // icon = self.render_key("icon") or ""

        // # winshortcut is a windows-only C extension! create_shortcut has this API
        // # Notice args must be passed as positional, no keywords allowed!
        // # winshortcut.create_shortcut(path, description, filename, arguments="",
        // #                             workdir=None, iconpath=None, iconindex=0, app_id="")
        // create_shortcut(
        //     target_path,
        //     self._shortcut_filename(ext=""),
        //     str(path),
        //     " ".join(arguments),
        //     working_dir,
        //     icon,
        //     0,
        //     self._app_user_model_id(),
        // )
        Ok(())
    }

    pub fn install(self) -> Result<(), MenuInstError> {
        // let paths = [
        //     Some(&self.directories.programs),
        //     if self.item.desktop.unwrap_or(false) {
        //         self.directories.desktop.as_ref()
        //     } else {
        //         None
        //     },
        //     if self.item.quicklaunch.unwrap_or(false) {
        //         self.directories.quicklaunch.as_ref()
        //     } else {
        //         None
        //     },
        // ];
        // for path in paths.into_iter().filter_map(identity) {
        //     let link_path = path.join(&self.name).with_extension(SHORTCUT_EXTENSION);
        //     let args = self.build_command_invocation();
        // }

        Ok(())
    }
}

pub(crate) fn install_menu_item(
    prefix: &Path,
    windows_item: Windows,
    command: MenuItemCommand,
    placeholders: &BaseMenuItemPlaceholders,
    menu_mode: MenuMode,
) -> Result<(), MenuInstError> {
    let menu = WindowsMenu::new(
        prefix,
        windows_item,
        command,
        Directories::create(),
        placeholders,
    );
    menu.install()
}

pub(crate) fn remove_menu_item(
    prefix: &Path,
    specific: Windows,
    command: MenuItemCommand,
    menu_mode: MenuMode,
) -> Result<(), MenuInstError> {
    todo!()
}
