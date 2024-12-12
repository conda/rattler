use fs_err as fs;
use std::{
    io::Write as _,
    path::{Path, PathBuf},
};

use crate::{
    render::{BaseMenuItemPlaceholders, MenuItemPlaceholders},
    schema::{Environment, MenuItemCommand, Windows},
    slugify, MenuInstError, MenuMode,
};

mod create_shortcut;
mod knownfolders;
mod lex;
mod registry;

use knownfolders::UserHandle;

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
        // let programs = known_folders
        //     .get_folder_path("programs", UserHandle::Current)
        //     .unwrap_or(known_folders.get_folder_path("programs", UserHandle::Common).unwrap());

        let programs = PathBuf::from("C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs");

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

    fn script_content(&self) -> String {
        let mut lines = vec![
            "@echo off".to_string(),
            ":: Script generated by conda/menuinst".to_string(),
        ];

        if let Some(pre_command_code) = self.command.precommand.as_ref() {
            lines.push(pre_command_code.resolve(&self.placeholders));
        }

        if self.command.activate.unwrap_or_default() {
            // TODO handle activation
        }

        let args: Vec<String> = self
            .command
            .command
            .iter()
            .map(|elem| elem.resolve(&self.placeholders))
            .collect();

        lines.push(lex::quote_args(&args).join(" "));

        lines.join("\n")
    }

    fn shortcut_filename(&self, ext: Option<String>) -> String {
        let env = if let Some(env_name) = self.placeholders.as_ref().get("ENV_NAME") {
            format!(" ({})", env_name)
        } else {
            "".to_string()
        };

        let ext = ext.unwrap_or_else(|| "lnk".to_string());
        format!("{}{}{}", self.name, env, ext)
    }

    fn write_script(&self, path: Option<PathBuf>) -> Result<(), MenuInstError> {
        let path =
            path.unwrap_or_else(|| self.prefix.join("Menu").join(self.shortcut_filename(None)));

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = fs::File::create(&path)?;
        file.write_all(self.script_content().as_bytes())?;

        Ok(())
    }

    fn build_command(&self, with_arg1: bool) -> Vec<String> {
        // TODO handle activation
        let mut command = Vec::new();
        for elem in self.command.command.iter() {
            command.push(elem.resolve(&self.placeholders));
        }

        if with_arg1 && !command.iter().any(|s| s.contains("%1")) {
            command.push("%1".to_string());
        }

        command
    }

    fn precreate(&self) -> Result<(), MenuInstError> {
        if let Some(precreate_code) = self.command.precreate.as_ref() {
            let precreate_code = precreate_code.resolve(&self.placeholders);
            // TODO run precreate code in a hidden window
            tracing::info!("Precreate code: {}", precreate_code);
        }
        Ok(())
    }

    fn app_id(&self) -> String {
        match self.item.app_user_model_id.as_ref() {
            Some(aumi) => aumi.resolve(&self.placeholders),
            None => format!(
                "Menuinst.{}",
                slugify(&self.name)
                    .replace(".", "")
                    .chars()
                    .take(128)
                    .collect::<String>()
            ),
        }
    }

    fn create_shortcut(&self, args: &[String]) -> Result<(), MenuInstError> {
        let icon = self
            .command
            .icon
            .as_ref()
            .map(|s| s.resolve(&self.placeholders));

        let workdir = if let Some(workdir) = &self.command.working_dir {
            workdir.resolve(&self.placeholders)
        } else {
            "%HOMEPATH%".to_string()
        };

        if workdir != "%HOMEPATH%" {
            fs::create_dir_all(&workdir)?;
        }

        let app_id = self.app_id();

        // split args into command and arguments
        let (command, args) = args.split_first().unwrap();
        let args = lex::quote_args(args).join(" ");

        let link_name = format!("{}.lnk", self.name);
        if self.item.desktop.unwrap_or(false) {
            let desktop_link_path = self.directories.desktop.join(&link_name);
            create_shortcut::create_shortcut(
                &command,
                &self.command.description.resolve(&self.placeholders),
                &desktop_link_path.to_string_lossy().to_string(),
                Some(&args),
                Some(&workdir),
                icon.as_deref(),
                Some(0),
                Some(&app_id),
            )
            .unwrap();
        }

        if self.item.quicklaunch.unwrap_or(false) && self.directories.quick_launch.is_dir() {
            let quicklaunch_link_path = self.directories.quick_launch.join(link_name);
            create_shortcut::create_shortcut(
                &self.name,
                &self.command.description.resolve(&self.placeholders),
                &quicklaunch_link_path.to_string_lossy().to_string(),
                Some(&args),
                Some(&workdir),
                icon.as_deref(),
                Some(0),
                Some(&app_id),
            )
            .unwrap();
        }

        Ok(())
    }

    pub fn install(self) -> Result<(), MenuInstError> {
        let args = self.build_command(false);
        self.create_shortcut(&args)?;
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
