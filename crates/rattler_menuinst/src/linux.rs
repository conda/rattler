use std::io::Write;
use std::path::Path;
use std::{fs::File, path::PathBuf};

use rattler_conda_types::Platform;
use rattler_shell::activation::{ActivationVariables, Activator};
use rattler_shell::shell;

use crate::render::{BaseMenuItemPlaceholders, MenuItemPlaceholders};
use crate::{
    schema::{Linux, MenuItemCommand},
    MenuInstError, MenuMode,
};

pub struct LinuxMenu {
    prefix: PathBuf,
    name: String,
    item: Linux,
    command: MenuItemCommand,
    directories: Directories,
    placeholders: MenuItemPlaceholders,
}

pub struct Directories {
    config_directory: PathBuf,
    data_directory: PathBuf,
    system_menu_config_location: PathBuf,
    desktop_entries_location: PathBuf,
}

impl Directories {
    fn new(mode: MenuMode) -> Self {
        let system_config_directory = PathBuf::from("/etc/xdg/");
        let system_data_directory = PathBuf::from("/usr/share");

        let (config_directory, data_directory) = if mode == MenuMode::System {
            (
                system_config_directory.clone(),
                system_data_directory.clone(),
            )
        } else {
            (
                PathBuf::from(
                    std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| "~/.config".to_string()),
                ),
                PathBuf::from(
                    std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| "~/.local/share".to_string()),
                ),
            )
        };

        Directories {
            config_directory: config_directory.clone(),
            data_directory: data_directory.clone(),
            system_menu_config_location: system_config_directory
                .join("menus")
                .join("applications.menu"),
            desktop_entries_location: data_directory.join("applications"),
        }
    }
}

impl LinuxMenu {
    fn new(
        prefix: &Path,
        item: Linux,
        command: MenuItemCommand,
        placeholders: &BaseMenuItemPlaceholders,
        mode: MenuMode,
    ) -> Self {
        let directories = Directories::new(mode);
        // TODO unsure if this is the right value for MENU_ITEM_LOCATION
        let refined_placeholders = placeholders.refine(&directories.system_menu_config_location);

        LinuxMenu {
            prefix: prefix.to_path_buf(),
            name: command
                .name
                .resolve(crate::schema::Environment::Base, &placeholders)
                .to_string(),
            item,
            command,
            directories,
            placeholders: refined_placeholders,
        }
    }

    fn location(&self) -> PathBuf {
        // TODO: The Python implementation uses one more variable
        let filename = format!("{}.desktop", self.name);
        self.directories.desktop_entries_location.join(filename)
    }

    /// Logic to run before the shortcut files are created.
    fn pre_create(&self) -> Result<(), MenuInstError> {
        if Platform::current().is_windows() {
            // TODO: return error
            return Ok(());
        }
        Ok(())
    }

    fn command(&self) -> String {
        let mut parts = Vec::new();
        if let Some(pre_command) = &self.command.precommand {
            parts.push(pre_command.resolve(&self.placeholders));
        }

        // TODO we should use `env` to set the environment variables in the `.desktop` file
        let mut envs = Vec::new();
        if self.command.activate.unwrap_or(false) {
            // create a bash activation script and emit it into the script
            let activator =
                Activator::from_path(&self.prefix, shell::Bash, Platform::current()).unwrap();
            let activation_env = activator
                .run_activation(ActivationVariables::default(), None)
                .unwrap();

            for (k, v) in activation_env {
                envs.push(format!(r#"{k}="{v}""#, k = k, v = v));
            }
            println!("Envs: {:?}", envs);
        }

        let command = self
            .command
            .command
            .iter()
            .map(|s| s.resolve(&self.placeholders))
            .collect::<Vec<_>>()
            .join(" ");

        parts.push(command);

        return parts.join(" && ");
    }

    fn create_desktop_entry(&self) -> Result<(), MenuInstError> {
        let file = self.location();
        let writer = File::create(file)?;
        let mut writer = std::io::BufWriter::new(writer);

        writeln!(writer, "[Desktop Entry]")?;
        writeln!(writer, "Type=Application")?;
        writeln!(writer, "Encoding=UTF-8")?;
        writeln!(writer, "Name={:?}", self.command.name)?;
        writeln!(writer, "Exec={}", self.command())?;
        writeln!(
            writer,
            "Terminal={}",
            self.command.terminal.unwrap_or(false)
        )?;

        if let Some(icon) = &self.command.icon {
            let icon = icon.resolve(&self.placeholders);
            writeln!(writer, "Icon={icon}")?;
        }

        let description = self.command.description.resolve(&self.placeholders);
        if !description.is_empty() {
            writeln!(writer, "Comment={}", description)?;
        }

        if let Some(working_dir) = &self.command.working_dir {
            let working_dir = working_dir.resolve(&self.placeholders);
            writeln!(writer, "Path={working_dir}")?;
        }

        // TODO write the rest of the stuff.
        // for key in menuitem_defaults["platforms"]["linux"]:
        //     if key in (*menuitem_defaults, "glob_patterns"):
        //         continue
        //     value = self.render_key(key)
        //     if value is None:
        //         continue
        //     if isinstance(value, bool):
        //         value = str(value).lower()
        //     elif isinstance(value, (list, tuple)):
        //         value = ";".join(value) + ";"
        //     lines.append(f"{key}={value}")

        Ok(())
    }

    fn register_mime_types(&self) -> Result<(), MenuInstError> {
        if self.item.mime_type.is_none() {
            return Ok(());
        }
        let mime_type = self.item.mime_type.as_ref().unwrap();
        if mime_type.is_empty() {
            return Ok(());
        }

        Ok(())
    }

    fn install(&self) -> Result<(), MenuInstError> {
        self.pre_create()?;
        self.create_desktop_entry()?;
        Ok(())
    }

    fn remove(&self) -> Result<(), MenuInstError> {
        Ok(())
    }
}

pub fn install_menu_item(
    prefix: &Path,
    item: Linux,
    command: MenuItemCommand,
    placeholders: &BaseMenuItemPlaceholders,
    menu_mode: MenuMode,
) -> Result<(), MenuInstError> {
    let menu = LinuxMenu::new(prefix, item, command, placeholders, menu_mode);
    menu.install()?;
    println!("{:?}", menu.location());
    println!("{:?}", menu.directories.config_directory);
    Ok(())
}