use fs_err as fs;
use fs_err::File;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

use rattler_conda_types::Platform;
use rattler_shell::activation::{ActivationVariables, Activator};
use rattler_shell::shell;

use crate::render::{BaseMenuItemPlaceholders, MenuItemPlaceholders, PlaceholderString};
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
    mode: MenuMode,
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
                dirs::home_dir().expect("Could not get home dir"),
                dirs::data_dir().expect("Could not get data dir"),
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
                .resolve(crate::schema::Environment::Base, placeholders),
            item,
            command,
            directories,
            placeholders: refined_placeholders,
            mode,
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
                envs.push(format!(r#"{k}="{v}""#));
            }
            println!("Envs: {envs:?}");
        }

        let command = self
            .command
            .command
            .iter()
            .map(|s| s.resolve(&self.placeholders))
            .collect::<Vec<_>>()
            .join(" ");

        parts.push(command);

        let command = parts.join(" && ");

        format!("bash -c {}", shlex::try_quote(&command).unwrap())
    }

    fn resolve_and_join(&self, items: &[PlaceholderString]) -> String {
        let mut res = String::new();
        for item in items {
            res.push_str(&item.resolve(&self.placeholders));
            res.push(';');
        }
        res
    }

    fn create_desktop_entry(&self) -> Result<(), MenuInstError> {
        let file = self.location();
        let writer = File::create(file)?;
        let mut writer = std::io::BufWriter::new(writer);

        writeln!(writer, "[Desktop Entry]")?;
        writeln!(writer, "Type=Application")?;
        writeln!(writer, "Encoding=UTF-8")?;
        writeln!(writer, "Name={}", self.name)?;
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
            writeln!(writer, "Comment={description}")?;
        }

        if let Some(working_dir) = &self.command.working_dir {
            let working_dir = working_dir.resolve(&self.placeholders);
            writeln!(writer, "Path={working_dir}")?;
        }

        // resolve categories and join them with a semicolon
        if let Some(categories) = &self.item.categories {
            writeln!(writer, "Categories={}", self.resolve_and_join(categories))?;
        }

        if let Some(dbus_activatable) = &self.item.dbus_activatable {
            writeln!(writer, "DBusActivatable={dbus_activatable}")?;
        }

        if let Some(generic_name) = &self.item.generic_name {
            writeln!(
                writer,
                "GenericName={}",
                generic_name.resolve(&self.placeholders)
            )?;
        }

        if let Some(hidden) = &self.item.hidden {
            writeln!(writer, "Hidden={hidden}")?;
        }

        if let Some(implements) = &self.item.implements {
            writeln!(writer, "Implements={}", self.resolve_and_join(implements))?;
        }

        if let Some(keywords) = &self.item.keywords {
            writeln!(writer, "Keywords={}", self.resolve_and_join(keywords))?;
        }

        if let Some(mime_types) = &self.item.mime_type {
            writeln!(writer, "MimeType={}", self.resolve_and_join(mime_types))?;
        }

        if let Some(no_display) = &self.item.no_display {
            writeln!(writer, "NoDisplay={no_display}")?;
        }

        if let Some(not_show_in) = &self.item.not_show_in {
            writeln!(writer, "NotShowIn={}", self.resolve_and_join(not_show_in))?;
        }

        if let Some(only_show_in) = &self.item.only_show_in {
            writeln!(writer, "OnlyShowIn={}", self.resolve_and_join(only_show_in))?;
        }

        if let Some(prefers_non_default_gpu) = &self.item.prefers_non_default_gpu {
            writeln!(writer, "PrefersNonDefaultGPU={prefers_non_default_gpu}")?;
        }

        if let Some(startup_notify) = &self.item.startup_notify {
            writeln!(writer, "StartupNotify={startup_notify}")?;
        }

        if let Some(startup_wm_class) = &self.item.startup_wm_class {
            writeln!(
                writer,
                "StartupWMClass={}",
                startup_wm_class.resolve(&self.placeholders)
            )?;
        }

        if let Some(try_exec) = &self.item.try_exec {
            writeln!(writer, "TryExec={}", try_exec.resolve(&self.placeholders))?;
        }

        Ok(())
    }

    fn update_desktop_database() -> Result<(), MenuInstError> {
        // We don't care about the output of update-desktop-database
        let _ = Command::new("update-desktop-database").output();

        Ok(())
    }

    fn install(&self) -> Result<(), MenuInstError> {
        self.pre_create()?;
        self.create_desktop_entry()?;
        self.maybe_register_mime_types(true)?;
        Self::update_desktop_database()?;
        Ok(())
    }

    fn remove(&self) -> Result<(), MenuInstError> {
        Ok(())
    }

    fn maybe_register_mime_types(&self, register: bool) -> Result<(), MenuInstError> {
        if let Some(mime_types) = self.item.mime_type.as_ref() {
            let resolved_mime_types = mime_types
                .iter()
                .map(|s| s.resolve(&self.placeholders))
                .collect::<Vec<String>>();
            self.register_mime_types(&resolved_mime_types, register)?;
        }
        Ok(())
    }

    fn register_mime_types(
        &self,
        mime_types: &[String],
        register: bool,
    ) -> Result<(), MenuInstError> {
        let mut resolved_globs = HashMap::<String, String>::new();

        if let Some(globs) = &self.item.glob_patterns {
            for (k, v) in globs {
                resolved_globs.insert(k.resolve(&self.placeholders), v.resolve(&self.placeholders));
            }
        }

        for mime_type in mime_types {
            if let Some(glob_pattern) = resolved_globs.get(mime_type) {
                self.glob_pattern_for_mime_type(mime_type, glob_pattern, register)?;
            }
        }

        if register {
            if let Some(xdg_mime) = which::which("xdg-mime").ok() {
                let mut command = Command::new(xdg_mime);
                command.arg("default").arg(&self.location());
                for mime_type in mime_types {
                    command.arg(mime_type);
                }
                let output = command.output()?;
                if !output.status.success() {
                    tracing::warn!(
                        "Could not set MIME type {} as default for {}",
                        mime_types.join(", "),
                        self.name
                    );
                    tracing::info!(
                        "xdg-mime stdout output: {}",
                        String::from_utf8_lossy(&output.stdout)
                    );
                    tracing::info!(
                        "xdg-mime stderr output: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
            } else {
                tracing::debug!("xdg-mime not found, not registering mime types as default.");
            }
        }

        // if let Some(update_mime_database) = which::which("update-mime-database").ok() {
        //     let mut command = Command::new(update_mime_database);
        //     command.arg("-V").arg(self.menu.data_directory.join("mime"));
        //     self.logged_run(&mut command)?;
        // }

        Ok(())
    }

    fn xml_path_for_mime_type(&self, mime_type: &str) -> (PathBuf, bool) {
        let basename = mime_type.replace("/", "-");
        let xml_files: Vec<PathBuf> =
            fs::read_dir(self.directories.data_directory.join("mime/applications"))
                .unwrap()
                .filter_map(|entry| {
                    let path = entry.unwrap().path();
                    if path
                        .file_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .contains(&basename)
                    {
                        Some(path)
                    } else {
                        None
                    }
                })
                .collect();

        if !xml_files.is_empty() {
            if xml_files.len() > 1 {
                tracing::debug!(
                    "Found multiple files for MIME type {}: {:?}. Returning first.",
                    mime_type,
                    xml_files
                );
            }
            return (xml_files[0].clone(), true);
        }
        (
            self.directories
                .data_directory
                .join("mime/packages")
                .join(format!("{basename}.xml")),
            false,
        )
    }

    fn glob_pattern_for_mime_type(
        &self,
        mime_type: &str,
        glob_pattern: &str,
        install: bool,
    ) -> Result<PathBuf, MenuInstError> {
        let (xml_path, exists) = self.xml_path_for_mime_type(mime_type);
        if exists {
            return Ok(xml_path);
        }

        // Write the XML that binds our current mime type to the glob pattern
        let xmlns = "http://www.freedesktop.org/standards/shared-mime-info";
        let description = format!(
            "Custom MIME type {mime_type} for '{glob_pattern}' files (registered by menuinst)"
        );

        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
    <mime-info xmlns="{xmlns}">
        <mime-type type="{mime_type}">
            <glob pattern="{glob_pattern}"/>
            <comment>{description}</comment>
        </mime-type>
    </mime-info>"#
        );

        let subcommand = if install { "install" } else { "uninstall" };
        // Install the XML file and register it as default for our app
        let tmp_dir = TempDir::new()?;
        let tmp_path = tmp_dir.path().join(xml_path.file_name().unwrap());
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(xml.as_bytes())?;

        let mut command = Command::new("xdg-mime");
        let mode = match self.mode {
            MenuMode::System => "system",
            MenuMode::User => "user",
        };

        command
            .arg(subcommand)
            .arg("--mode")
            .arg(mode)
            .arg("--novendor")
            .arg(tmp_path);
        let output = command.output()?;

        if !output.status.success() {
            tracing::warn!(
                "Could not un/register MIME type {} with xdg-mime. Writing to '{}' as a fallback.",
                mime_type,
                xml_path.display()
            );
            tracing::info!(
                "xdg-mime stdout output: {}",
                String::from_utf8_lossy(&output.stdout)
            );
            tracing::info!(
                "xdg-mime stderr output: {}",
                String::from_utf8_lossy(&output.stderr)
            );

            let mut file = fs::File::create(&xml_path)?;
            file.write_all(xml.as_bytes())?;
        }

        Ok(xml_path)
    }

    /// All paths that are installed for removal
    fn paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.location()];

        if let Some(mime_types) = &self.item.mime_type {
            let resolved = mime_types
                .iter()
                .map(|s| s.resolve(&self.placeholders))
                .collect::<Vec<String>>();

            for mime in resolved {
                let (xml_path, exists) = self.xml_path_for_mime_type(&mime);
                if !exists {
                    continue;
                }

                if let Ok(content) = fs::read_to_string(&xml_path) {
                    if content.contains("registered by menuinst") {
                        paths.push(xml_path);
                    }
                }
            }
        }
        paths
    }
}

/// Install a menu item on Linux.
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
