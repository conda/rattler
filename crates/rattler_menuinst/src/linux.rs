use fs_err as fs;
use fs_err::File;
use mime_config::MimeConfig;
use rattler_conda_types::menuinst::{LinuxRegisteredMimeFile, LinuxTracker, MenuMode};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

mod mime_config;

use rattler_conda_types::Platform;
use rattler_shell::activation::{ActivationVariables, Activator};
use rattler_shell::shell;

use crate::render::{BaseMenuItemPlaceholders, MenuItemPlaceholders, PlaceholderString};
use crate::utils::{log_output, run_pre_create_command, slugify};
use crate::{
    schema::{Linux, MenuItemCommand},
    MenuInstError,
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

    /// The location of the menu configuration file.
    /// This is the file that is used by the system to determine the menu layout
    /// It is usually located at `~/.config/menus/applications.menu`
    pub menu_config_location: PathBuf,

    /// The location of the system menu configuration file
    /// This is the file that is used by the system to determine the menu layout.
    /// It is usually located at `/etc/xdg/menus/applications.menu`
    pub system_menu_config_location: PathBuf,

    /// The location of the desktop entries
    /// This is a directory that contains `.desktop` files
    /// that describe the applications that are shown in the menu.
    /// It is usually located at `/usr/share/applications` or
    /// `~/.local/share/applications`
    pub desktop_entries_location: PathBuf,

    /// The location of the desktop-directories
    /// This is a directory that contains `.directory` files
    /// that describe the directories that are shown in the menu
    /// It is usually located at `/usr/share/desktop-directories` or
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
                dirs::config_dir().expect("could not get config dir"),
                dirs::data_dir().expect("could not get data dir"),
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
            self.menu_config_location
                .parent()
                .ok_or_else(|| MenuInstError::InvalidPath(self.menu_config_location.clone()))?,
            &self.desktop_entries_location,
            self.directory_entry_location
                .parent()
                .ok_or_else(|| MenuInstError::InvalidPath(self.directory_entry_location.clone()))?,
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

/// Update the mime database for a given directory
fn update_mime_database(directory: &Path) -> Result<(), MenuInstError> {
    if let Ok(update_mime_database) = which::which("update-mime-database") {
        let output = Command::new(update_mime_database)
            .arg("-V")
            .arg(directory)
            .output()?;

        if !output.status.success() {
            tracing::warn!("Could not update mime database");
            log_output("update-mime-database", output);
        }
    }
    Ok(())
}

enum XdgMimeOperation {
    Install,
    Uninstall,
}

/// XDG Mime invocation error
#[derive(Debug, thiserror::Error)]
pub enum XdgMimeError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Run `xdg-mime` with the given arguments
fn xdg_mime(
    xml_file: &Path,
    mode: MenuMode,
    operation: XdgMimeOperation,
) -> Result<(), XdgMimeError> {
    let mut command = Command::new("xdg-mime");

    let mode = match mode {
        MenuMode::System => "system",
        MenuMode::User => "user",
    };

    let operation = match operation {
        XdgMimeOperation::Install => "install",
        XdgMimeOperation::Uninstall => "uninstall",
    };

    command
        .arg(operation)
        .arg("--mode")
        .arg(mode)
        .arg("--novendor")
        .arg(xml_file);

    let output = command.output()?;

    if !output.status.success() {
        tracing::warn!(
            "Could not un/register MIME type with xdg-mime. Writing to '{}' as a fallback.",
            xml_file.display()
        );
        log_output("xdg-mime", output);

        return Err(XdgMimeError::IoError(std::io::Error::new(
            std::io::ErrorKind::Other,
            "xdg-mime failed",
        )));
    }

    Ok(())
}

/// Update the desktop database by running `update-desktop-database`
fn update_desktop_database() -> Result<(), MenuInstError> {
    // We don't care about the output of update-desktop-database
    let _ = Command::new("update-desktop-database").output();

    Ok(())
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

    fn command(&self) -> Result<String, MenuInstError> {
        let mut parts = Vec::new();
        if let Some(pre_command) = &self.command.precommand {
            parts.push(pre_command.resolve(&self.placeholders));
        }

        let mut envs = Vec::new();
        if self.command.activate.unwrap_or(false) {
            // create a bash activation script and emit it into the script
            let activator = Activator::from_path(&self.prefix, shell::Bash, Platform::current())?;
            let activation_env = activator.run_activation(ActivationVariables::default(), None)?;

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
        let bash_command = format!("bash -c {}", shlex::try_quote(&command)?);
        if envs.is_empty() {
            Ok(bash_command)
        } else {
            Ok(format!("env {} {}", envs.join(" "), bash_command))
        }
    }

    fn resolve_and_join(&self, items: &[PlaceholderString]) -> String {
        let mut res = String::new();
        for item in items {
            res.push_str(&item.resolve(&self.placeholders));
            res.push(';');
        }
        res
    }

    fn create_desktop_entry(&self, tracker: &mut LinuxTracker) -> Result<(), MenuInstError> {
        let file = self.location();
        tracing::info!("Creating desktop entry at {}", file.display());
        let writer = File::create(&file)?;
        let mut writer = std::io::BufWriter::new(writer);

        tracker.paths.push(file);

        writeln!(writer, "[Desktop Entry]")?;
        writeln!(writer, "Type=Application")?;
        writeln!(writer, "Encoding=UTF-8")?;
        writeln!(writer, "Name={}", self.name)?;
        writeln!(writer, "Exec={}", self.command()?)?;
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

    fn install(&self, tracker: &mut LinuxTracker) -> Result<(), MenuInstError> {
        self.directories.ensure_directories_exist()?;
        self.pre_create()?;
        self.create_desktop_entry(tracker)?;
        self.register_mime_types(tracker)?;
        update_desktop_database()?;
        Ok(())
    }

    fn register_mime_types(&self, tracker: &mut LinuxTracker) -> Result<(), MenuInstError> {
        let Some(mime_types) = self.item.mime_type.as_ref() else {
            return Ok(());
        };

        let mime_types = mime_types
            .iter()
            .map(|s| s.resolve(&self.placeholders))
            .collect::<Vec<String>>();

        tracing::info!("Registering mime types {:?}", mime_types);
        let mut resolved_globs = HashMap::<String, String>::new();

        if let Some(globs) = &self.item.glob_patterns {
            for (k, v) in globs {
                resolved_globs.insert(k.resolve(&self.placeholders), v.resolve(&self.placeholders));
            }
        }

        for mime_type in &mime_types {
            if let Some(glob_pattern) = resolved_globs.get(mime_type) {
                self.glob_pattern_for_mime_type(mime_type, glob_pattern, tracker)?;
            }
        }

        let mimeapps = self.directories.config_directory.join("mimeapps.list");

        let mut config = MimeConfig::load(&mimeapps)?;
        for mime_type in &mime_types {
            tracing::info!("Registering mime type {} for {}", mime_type, &self.name);
            config.register_mime_type(mime_type, &self.name);
        }
        config.save()?;

        // Store the data so that we can remove it later
        tracker.mime_types = Some(LinuxRegisteredMimeFile {
            mime_types: mime_types.clone(),
            database_path: self.directories.mime_directory(),
            application: self.name.clone(),
            config_file: mimeapps,
        });

        update_mime_database(&self.directories.mime_directory())?;

        Ok(())
    }

    fn xml_path_for_mime_type(&self, mime_type: &str) -> Result<(PathBuf, bool), std::io::Error> {
        let basename = mime_type.replace("/", "-");
        let mime_directory = self.directories.data_directory.join("mime/packages");
        if !mime_directory.is_dir() {
            return Ok((mime_directory.join(format!("{basename}.xml")), false));
        }

        let xml_files: Vec<PathBuf> = fs::read_dir(&mime_directory)?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                let file_name = path.file_name()?.to_str()?;
                if file_name.contains(&basename) {
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
        Ok((mime_directory.join(format!("{basename}.xml")), false))
    }

    fn glob_pattern_for_mime_type(
        &self,
        mime_type: &str,
        glob_pattern: &str,
        tracker: &mut LinuxTracker,
    ) -> Result<(), MenuInstError> {
        let (xml_path, exists) = self.xml_path_for_mime_type(mime_type)?;
        if exists {
            // Already registered this mime type
            return Ok(());
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

        // Install the XML file and register it as default for our app
        let file_name = xml_path.file_name().expect("we should have a filename");
        let mime_dir_in_prefix = self.prefix.join("Menu/registered-mimetypes");
        fs::create_dir_all(&mime_dir_in_prefix)?;

        // We store the mime file in a temporary dir inside the prefix, but we actually keep
        // the file + directory around for eventual removal (just use tempdir for unique folder)
        let temp_dir = TempDir::new_in(&mime_dir_in_prefix)?;
        let file_path = temp_dir.path().join(file_name);
        fs::write(&file_path, &xml)?;

        if xdg_mime(&file_path, self.mode, XdgMimeOperation::Install).is_ok() {
            // keep temp dir in prefix around and the temp file
            // because we re-use it when unregistering the mime type.
            let _ = temp_dir.into_path();
            tracker.registered_mime_files.push(file_path);
        } else {
            if let Some(parent) = xml_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&xml_path, xml)?;
            tracker.paths.push(xml_path);
        }

        Ok(())
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
) -> Result<LinuxTracker, MenuInstError> {
    let mut tracker = LinuxTracker::default();
    let menu = LinuxMenu::new(menu_name, prefix, item, command, placeholders, menu_mode);
    menu.install(&mut tracker)?;

    Ok(tracker)
}

/// Remove a menu item on Linux.
pub fn remove_menu_item(tracker: &LinuxTracker) -> Result<(), MenuInstError> {
    for path in &tracker.paths {
        match fs::remove_file(path) {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("Could not remove file {}: {}", path.display(), e);
            }
        }
    }

    for path in &tracker.registered_mime_files {
        match xdg_mime(path, tracker.install_mode, XdgMimeOperation::Uninstall) {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("Could not uninstall mime type: {}", e);
            }
        }
        // Remove the temporary directory we created for the glob file
        if let Some(parent) = path.parent() {
            fs::remove_dir_all(parent)?;
        }
    }

    if let Some(installed_mime_types) = tracker.mime_types.as_ref() {
        // load mimetype config
        let mut config = MimeConfig::load(&installed_mime_types.config_file)?;
        let application = &installed_mime_types.application;
        for mime_type in &installed_mime_types.mime_types {
            config.deregister_mime_type(mime_type, application);
        }
        config.save()?;

        update_mime_database(&installed_mime_types.database_path)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use fs_err as fs;
    use rattler_conda_types::menuinst::LinuxTracker;
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

            std::env::set_var("XDG_DATA_HOME", &data_directory);
            std::env::set_var("XDG_CONFIG_HOME", &config_directory);

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

    impl Drop for FakeDirectories {
        fn drop(&mut self) {
            std::env::remove_var("XDG_DATA_HOME");
            std::env::remove_var("XDG_CONFIG_HOME");
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
                    placeholders.insert("ICON_EXT".to_string(), (*ext).to_string());
                    let icon_path = icon.resolve(FakePlaceholders {
                        placeholders: placeholders.clone(),
                    });
                    fs::write(&icon_path, []).unwrap();
                }
            }

            fs::create_dir_all(prefix_path.join("bin")).unwrap();
            fs::write(prefix_path.join("bin/python"), []).unwrap();

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

        let mut tracker = LinuxTracker::default();
        linux_menu.install(&mut tracker).unwrap();

        // check snapshot of desktop file
        let desktop_file = dirs.directories().desktop_file();
        let desktop_file_content = fs::read_to_string(&desktop_file).unwrap();
        let desktop_file_content =
            desktop_file_content.replace(fake_prefix.prefix().to_str().unwrap(), "<PREFIX>");
        insta::assert_snapshot!(desktop_file_content);

        // check mimeapps.list
        let mimeapps_file = dirs.directories().config_directory.join("mimeapps.list");
        let mimeapps_file_content = fs::read_to_string(&mimeapps_file).unwrap();
        insta::assert_snapshot!(mimeapps_file_content);

        let mime_file = dirs
            .directories()
            .data_directory
            .join("mime/packages/text-x-spython.xml");
        let mime_file_content = fs::read_to_string(&mime_file).unwrap();
        insta::assert_snapshot!(mime_file_content);
    }
}
