use fs_err as fs;
use fs_err::File;
use mime_config::MimeConfig;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

mod menu_xml;
mod mime_config;

use rattler_conda_types::Platform;
use rattler_shell::activation::{ActivationVariables, Activator};
use rattler_shell::shell;

use crate::render::{BaseMenuItemPlaceholders, MenuItemPlaceholders, PlaceholderString};
use crate::slugify;
use crate::util::{log_output, run_pre_create_command};
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

/// Directories used on Linux for menu items
#[allow(unused)]
#[derive(Debug, Clone)]
pub struct Directories {
    /// The name of the (parent) menu (in the json defined as `menu_name`)
    pub menu_name: String,

    /// The name of the current menu item
    pub name: String,

    /// The data directory for the menu
    pub data_directory: PathBuf,

    /// The configuration directory for the menu
    pub config_directory: PathBuf,

    /// The location of the menu configuration file This is the file that is
    /// used by the system to determine the menu layout It is usually located at
    /// `~/.config/menus/applications.menu`
    pub menu_config_location: PathBuf,

    /// The location of the system menu configuration file This is the file that
    /// is used by the system to determine the menu layout It is usually located
    /// at `/etc/xdg/menus/applications.menu`
    pub system_menu_config_location: PathBuf,

    /// The location of the desktop entries This is a directory that contains
    /// `.desktop` files that describe the applications that are shown in the
    /// menu It is usually located at `/usr/share/applications` or
    /// `~/.local/share/applications`
    pub desktop_entries_location: PathBuf,

    /// The location of the desktop-directories This is a directory that
    /// contains `.directory` files that describe the directories that are shown
    /// in the menu It is usually located at `/usr/share/desktop-directories` or
    /// `~/.local/share/desktop-directories`
    pub directory_entry_location: PathBuf,
}

impl Directories {
    fn new(mode: MenuMode, menu_name: &str, name: &str) -> Self {
        let system_config_directory = PathBuf::from("/etc/xdg/");
        let system_data_directory = PathBuf::from("/usr/share");

        let (config_directory, data_directory) = if mode == MenuMode::System {
            (
                system_config_directory.clone(),
                system_data_directory.clone(),
            )
        } else {
            (
                dirs::config_dir().expect("Could not get config dir"),
                dirs::data_dir().expect("Could not get data dir"),
            )
        };

        Directories {
            menu_name: menu_name.to_string(),
            name: name.to_string(),
            data_directory: data_directory.clone(),
            system_menu_config_location: system_config_directory.join("menus/applications.menu"),
            menu_config_location: config_directory.join("menus/applications.menu"),
            config_directory,
            desktop_entries_location: data_directory.join("applications"),
            directory_entry_location: data_directory
                .join(format!("desktop-directories/{}.directory", slugify(name))),
        }
    }

    pub fn ensure_directories_exist(&self) -> Result<(), MenuInstError> {
        fs::create_dir_all(&self.data_directory)?;
        fs::create_dir_all(&self.config_directory)?;

        let paths = vec![
            self.menu_config_location.parent().unwrap().to_path_buf(),
            self.desktop_entries_location.clone(),
            self.directory_entry_location
                .parent()
                .unwrap()
                .to_path_buf(),
        ];

        for path in paths {
            tracing::debug!("Ensuring path {} exists", path.display());
            fs::create_dir_all(path)?;
        }

        Ok(())
    }

    pub fn mime_directory(&self) -> PathBuf {
        self.data_directory.join("mime")
    }

    pub fn desktop_file(&self) -> PathBuf {
        self.desktop_entries_location.join(format!(
            "{}_{}.desktop",
            slugify(&self.menu_name),
            slugify(&self.name)
        ))
    }
}

impl LinuxMenu {
    fn new(
        menu_name: &str,
        prefix: &Path,
        item: Linux,
        command: MenuItemCommand,
        placeholders: &BaseMenuItemPlaceholders,
        mode: MenuMode,
    ) -> Self {
        Self::new_impl(menu_name, prefix, item, command, placeholders, mode, None)
    }

    pub fn new_impl(
        menu_name: &str,
        prefix: &Path,
        item: Linux,
        command: MenuItemCommand,
        placeholders: &BaseMenuItemPlaceholders,
        mode: MenuMode,
        directories: Option<Directories>,
    ) -> Self {
        let name = command
            .name
            .resolve(crate::schema::Environment::Base, placeholders);

        let directories = directories.unwrap_or_else(|| Directories::new(mode, menu_name, &name));

        let refined_placeholders = placeholders.refine(&directories.desktop_file());

        LinuxMenu {
            name,
            prefix: prefix.to_path_buf(),
            item,
            command,
            directories,
            placeholders: refined_placeholders,
            mode,
        }
    }

    #[cfg(test)]
    pub fn new_with_directories(
        menu_name: &str,
        prefix: &Path,
        item: Linux,
        command: MenuItemCommand,
        placeholders: &BaseMenuItemPlaceholders,
        directories: Directories,
    ) -> Self {
        Self::new_impl(
            menu_name,
            prefix,
            item,
            command,
            placeholders,
            MenuMode::User,
            Some(directories),
        )
    }

    fn location(&self) -> PathBuf {
        self.directories.desktop_file()
    }

    /// Logic to run before the shortcut files are created.
    fn pre_create(&self) -> Result<(), MenuInstError> {
        if let Some(pre_create_command) = &self.command.precreate {
            let pre_create_command = pre_create_command.resolve(&self.placeholders);
            run_pre_create_command(&pre_create_command)?;
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
        tracing::info!("Creating desktop entry at {}", file.display());
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

        if let Some(single_main_window) = &self.item.single_main_window {
            writeln!(writer, "SingleMainWindow={single_main_window}")?;
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
        self.directories.ensure_directories_exist()?;
        self.pre_create()?;
        self.create_desktop_entry()?;
        self.maybe_register_mime_types(true)?;
        Self::update_desktop_database()?;
        Ok(())
    }

    fn remove(&self) -> Result<(), MenuInstError> {
        let paths = self.paths();
        for path in paths {
            fs::remove_file(path)?;
        }
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
        tracing::info!("Registering mime types {:?}", mime_types);
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

        let mimeapps = self.directories.config_directory.join("mimeapps.list");

        if register {
            let mut config = MimeConfig::new(mimeapps);
            config.load()?;
            for mime_type in mime_types {
                tracing::info!("Registering mime type {} for {}", mime_type, &self.name);
                config.register_mime_type(mime_type, &self.name);
            }
            config.save()?;
        } else if mimeapps.exists() {
            // in this case we remove the mime type from the mimeapps.list file
            let mut config = MimeConfig::new(mimeapps);
            for mime_type in mime_types {
                tracing::info!("Deregistering mime type {} for {}", mime_type, &self.name);
                config.deregister_mime_type(mime_type, &self.name);
            }
            config.save()?;
        }

        if let Ok(update_mime_database) = which::which("update-mime-database") {
            let mut command = Command::new(update_mime_database);
            command
                .arg("-V")
                .arg(self.directories.mime_directory());
            let output = command.output()?;
            if !output.status.success() {
                tracing::warn!("Could not update mime database");
                log_output("update-mime-database", output);
            }
        }

        Ok(())
    }

    fn xml_path_for_mime_type(&self, mime_type: &str) -> Result<(PathBuf, bool), std::io::Error> {
        let basename = mime_type.replace("/", "-");
        let mime_directory = self.directories.data_directory.join("mime/packages");
        if !mime_directory.is_dir() {
            return Ok((
                mime_directory.join(format!("{basename}.xml")),
                false,
            ));
        }

        let xml_files: Vec<PathBuf> = fs::read_dir(&mime_directory)?
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
            return Ok((xml_files[0].clone(), true));
        }
        Ok((
            mime_directory.join(format!("{basename}.xml")),
            false,
        ))
    }

    fn glob_pattern_for_mime_type(
        &self,
        mime_type: &str,
        glob_pattern: &str,
        install: bool,
    ) -> Result<PathBuf, MenuInstError> {
        let (xml_path, exists) = self.xml_path_for_mime_type(mime_type).unwrap();
        if exists {
            println!("XML path exists");
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
                let (xml_path, exists) = self.xml_path_for_mime_type(&mime).unwrap();
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
    menu_name: &str,
    prefix: &Path,
    item: Linux,
    command: MenuItemCommand,
    placeholders: &BaseMenuItemPlaceholders,
    menu_mode: MenuMode,
) -> Result<(), MenuInstError> {
    let menu = LinuxMenu::new(menu_name, prefix, item, command, placeholders, menu_mode);
    menu.install()
}

/// Remove a menu item on Linux.
pub fn remove_menu_item(
    menu_name: &str,
    prefix: &Path,
    item: Linux,
    command: MenuItemCommand,
    placeholders: &BaseMenuItemPlaceholders,
    menu_mode: MenuMode,
) -> Result<(), MenuInstError> {
    let menu = LinuxMenu::new(menu_name, prefix, item, command, placeholders, menu_mode);
    menu.remove()
}

#[cfg(test)]
mod tests {
    use fs_err as fs;
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
    };
    use tempfile::TempDir;

    use crate::{schema::MenuInstSchema, test::test_data};

    use super::{Directories, LinuxMenu};

    struct FakeDirectories {
        _tmp_dir: TempDir,
        directories: Directories,
    }

    impl FakeDirectories {
        fn new() -> Self {
            let tmp_dir = TempDir::new().unwrap();
            let data_directory = tmp_dir.path().join("data");
            let config_directory = tmp_dir.path().join("config");

            let directories = Directories {
                menu_name: "Test".to_string(),
                name: "Test".to_string(),
                data_directory,
                config_directory,
                system_menu_config_location: tmp_dir.path().join("system_menu_config_location"),
                menu_config_location: tmp_dir.path().join("menu_config_location"),
                desktop_entries_location: tmp_dir.path().join("desktop_entries_location"),
                directory_entry_location: tmp_dir.path().join("directory_entry_location"),
            };

            directories.ensure_directories_exist().unwrap();

            Self {
                _tmp_dir: tmp_dir,
                directories,
            }
        }

        pub fn directories(&self) -> &Directories {
            &self.directories
        }
    }

    struct FakePlaceholders {
        placeholders: HashMap<String, String>,
    }

    impl AsRef<HashMap<String, String>> for FakePlaceholders {
        fn as_ref(&self) -> &HashMap<String, String> {
            &self.placeholders
        }
    }

    struct FakePrefix {
        _tmp_dir: TempDir,
        prefix_path: PathBuf,
        schema: MenuInstSchema,
    }

    impl FakePrefix {
        fn new(schema_json: &str) -> Self {
            let tmp_dir = TempDir::new().unwrap();
            let prefix_path = tmp_dir.path().join("test-env");
            let schema_json = test_data().join(schema_json);
            let menu_folder = prefix_path.join("Menu");

            fs::create_dir_all(&menu_folder).unwrap();
            fs::copy(
                &schema_json,
                menu_folder.join(schema_json.file_name().unwrap()),
            )
            .unwrap();

            // Create a icon file for the
            let schema = std::fs::read_to_string(schema_json).unwrap();
            let parsed_schema: MenuInstSchema = serde_json::from_str(&schema).unwrap();

            let mut placeholders = HashMap::<String, String>::new();
            placeholders.insert(
                "MENU_DIR".to_string(),
                menu_folder.to_string_lossy().to_string(),
            );

            for item in &parsed_schema.menu_items {
                let icon = item.command.icon.as_ref().unwrap();
                for ext in &["icns", "png", "svg"] {
                    placeholders.insert("ICON_EXT".to_string(), ext.to_string());
                    let icon_path = icon.resolve(FakePlaceholders {
                        placeholders: placeholders.clone(),
                    });
                    fs::write(&icon_path, []).unwrap();
                }
            }

            fs::create_dir_all(prefix_path.join("bin")).unwrap();
            fs::write(prefix_path.join("bin/python"), &[]).unwrap();

            Self {
                _tmp_dir: tmp_dir,
                prefix_path,
                schema: parsed_schema,
            }
        }

        pub fn prefix(&self) -> &Path {
            &self.prefix_path
        }
    }

    // print the whole tree of a directory
    fn print_tree(path: &Path) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            println!("{}", path.display());
            if path.is_dir() {
                print_tree(&path);
            }
        }
    }


    #[test]
    fn test_installation() {
        let dirs = FakeDirectories::new();

        let fake_prefix = FakePrefix::new("spyder/menu.json");

        let item = fake_prefix.schema.menu_items[0].clone();
        let linux = item.platforms.linux.unwrap();
        let command = item.command.merge(linux.base);

        let placeholders = super::BaseMenuItemPlaceholders::new(
            fake_prefix.prefix(),
            fake_prefix.prefix(),
            rattler_conda_types::Platform::current(),
        );

        let linux_menu = LinuxMenu::new_with_directories(
            &fake_prefix.schema.menu_name,
            fake_prefix.prefix(),
            linux.specific,
            command,
            &placeholders,
            dirs.directories().clone(),
        );

        linux_menu.install().unwrap();
        print_tree(&dirs.directories().config_directory);
        // check snapshot of desktop file
        let desktop_file = dirs.directories().desktop_file();
        let desktop_file_content = fs::read_to_string(&desktop_file).unwrap();
        let desktop_file_content =
            desktop_file_content.replace(&fake_prefix.prefix().to_str().unwrap(), "<PREFIX>");
        insta::assert_snapshot!(desktop_file_content);

        // check mimeapps.list
        let mimeapps_file = dirs.directories().config_directory.join("mimeapps.list");
        let mimeapps_file_content = fs::read_to_string(&mimeapps_file).unwrap();
        insta::assert_snapshot!(mimeapps_file_content);

        let mime_file = dirs::data_dir().unwrap().join("mime/packages/text-x-spython.xml");
        let mime_file_content = fs::read_to_string(&mime_file).unwrap();
        insta::assert_snapshot!(mime_file_content);

    }
}