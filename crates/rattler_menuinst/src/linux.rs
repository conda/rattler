use std::io::Write;
use std::path::Path;
use std::{fs::File, path::PathBuf};

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

        // resolve categories and join them with a semicolon
        if let Some(categories) = &self.item.categories {
            writeln!(writer, "Categories={}", self.resolve_and_join(categories))?;
        }

        if let Some(dbus_activatable) = &self.item.dbus_activatable {
            writeln!(writer, "DBusActivatable={}", dbus_activatable)?;
        }

        if let Some(generic_name) = &self.item.generic_name {
            writeln!(
                writer,
                "GenericName={}",
                generic_name.resolve(&self.placeholders)
            )?;
        }

        if let Some(hidden) = &self.item.hidden {
            writeln!(writer, "Hidden={}", hidden)?;
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
            writeln!(writer, "NoDisplay={}", no_display)?;
        }

        if let Some(not_show_in) = &self.item.not_show_in {
            writeln!(writer, "NotShowIn={}", self.resolve_and_join(not_show_in))?;
        }

        if let Some(only_show_in) = &self.item.only_show_in {
            writeln!(writer, "OnlyShowIn={}", self.resolve_and_join(only_show_in))?;
        }

        if let Some(prefers_non_default_gpu) = &self.item.prefers_non_default_gpu {
            writeln!(writer, "PrefersNonDefaultGPU={}", prefers_non_default_gpu)?;
        }

        if let Some(startup_notify) = &self.item.startup_notify {
            writeln!(writer, "StartupNotify={}", startup_notify)?;
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

    //     fn maybe_register_mime_types(&self, register: bool) -> Result<(), MenuInstError> {
    //         if let Some(mime_types) = self.command.mime_type.as_ref().map(|s| s.resolve(&self.placeholders)) {
    //             self.register_mime_types(mime_types.split(';').collect(), register)?;
    //         }
    //         Ok(())
    //     }

    //     fn register_mime_types(&self, mime_types: Vec<&str>, register: bool) -> Result<(), MenuInstError> {
    //         let glob_patterns: HashMap<String, String> = self.command.glob_patterns.as_ref().map(|s| s.resolve(&self.placeholders)).unwrap_or_default();
    //         for mime_type in mime_types {
    //             if let Some(glob_pattern) = glob_patterns.get(mime_type) {
    //                 self.glob_pattern_for_mime_type(mime_type, glob_pattern, register)?;
    //             }
    //         }

    //         if register {
    //             if let Some(xdg_mime) = which::which("xdg-mime").ok() {
    //                 let mut command = Command::new(xdg_mime);
    //                 command.arg("default").arg(&self.location());
    //                 for mime_type in &mime_types {
    //                     command.arg(mime_type);
    //                 }
    //                 self.logged_run(&mut command)?;
    //             } else {
    //                 log::debug!("xdg-mime not found, not registering mime types as default.");
    //             }
    //         }

    //         if let Some(update_mime_database) = which::which("update-mime-database").ok() {
    //             let mut command = Command::new(update_mime_database);
    //             command.arg("-V").arg(self.menu.data_directory.join("mime"));
    //             self.logged_run(&mut command)?;
    //         }

    //         Ok(())
    //     }

    //     fn xml_path_for_mime_type(&self, mime_type: &str) -> (PathBuf, bool) {
    //         let basename = mime_type.replace("/", "-");
    //         let xml_files: Vec<PathBuf> = fs::read_dir(self.menu.data_directory.join("mime/applications"))
    //             .unwrap()
    //             .filter_map(|entry| {
    //                 let path = entry.unwrap().path();
    //                 if path.file_name().unwrap().to_str().unwrap().contains(&basename) {
    //                     Some(path)
    //                 } else {
    //                     None
    //                 }
    //             })
    //             .collect();

    //         if !xml_files.is_empty() {
    //             if xml_files.len() > 1 {
    //                 log::debug!("Found multiple files for MIME type {}: {:?}. Returning first.", mime_type, xml_files);
    //             }
    //             return (xml_files[0].clone(), true);
    //         }
    //         (self.menu.data_directory.join("mime/packages").join(format!("{}.xml", basename)), false)
    //     }

    //     fn glob_pattern_for_mime_type(&self, mime_type: &str, glob_pattern: &str, install: bool) -> Result<PathBuf, MenuInstError> {
    //         let (xml_path, exists) = self.xml_path_for_mime_type(mime_type);
    //         if exists {
    //             return Ok(xml_path);
    //         }

    //         // Write the XML that binds our current mime type to the glob pattern
    //         let xmlns = "http://www.freedesktop.org/standards/shared-mime-info";
    //         let mut mime_info = Element::new("mime-info");
    //         mime_info.attributes.insert("xmlns".to_string(), xmlns.to_string());

    //         let mut mime_type_tag = Element::new("mime-type");
    //         mime_type_tag.attributes.insert("type".to_string(), mime_type.to_string());

    //         let mut glob = Element::new("glob");
    //         glob.attributes.insert("pattern".to_string(), glob_pattern.to_string());
    //         mime_type_tag.children.push(XMLNode::Element(glob));

    //         let descr = format!("Custom MIME type {} for '{}' files (registered by menuinst)", mime_type, glob_pattern);
    //         let mut comment = Element::new("comment");
    //         comment.children.push(XMLNode::Text(descr));
    //         mime_type_tag.children.push(XMLNode::Element(comment));

    //         mime_info.children.push(XMLNode::Element(mime_type_tag));
    //         let tree = Element::new("mime-info");
    //         tree.children.push(XMLNode::Element(mime_info));

    //         let subcommand = if install { "install" } else { "uninstall" };
    //         // Install the XML file and register it as default for our app
    //         let tmp_dir = TempDir::new()?;
    //         let tmp_path = tmp_dir.path().join(xml_path.file_name().unwrap());
    //         let mut file = fs::File::create(&tmp_path)?;
    //         tree.write(&mut file)?;

    //         let mut command = Command::new("xdg-mime");
    //         command.arg(subcommand).arg("--mode").arg(&self.menu.mode).arg("--novendor").arg(tmp_path);
    //         if let Err(_) = self.logged_run(&mut command) {
    //             log::debug!("Could not un/register MIME type {} with xdg-mime. Writing to '{}' as a fallback.", mime_type, xml_path.display());
    //             let mut file = fs::File::create(&xml_path)?;
    //             tree.write(&mut file)?;
    //         }

    //         Ok(xml_path)
    //     }

    //     fn paths(&self) -> Vec<PathBuf> {
    //         let mut paths = vec![self.location()];
    //         if let Some(mime_types) = self.command.mime_type.as_ref().map(|s| s.resolve(&self.placeholders)) {
    //             for mime in mime_types.split(';') {
    //                 let (xml_path, exists) = self.xml_path_for_mime_type(mime);
    //                 if exists && fs::read_to_string(&xml_path).unwrap().contains("registered by menuinst") {
    //                     paths.push(xml_path);
    //                 }
    //             }
    //         }
    //         paths
    //    }
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