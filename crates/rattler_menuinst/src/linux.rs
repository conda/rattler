// use std::collections::HashMap;
// use std::env;
// use std::fs::{self, File};
use std::io::Write;
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
// use std::path::{Path, PathBuf};
// use std::process::Command;

// use chrono::Local;
// use log::{debug, warn};
// use regex::Regex;
// use serde::{Deserialize, Serialize};
// use tempfile::TempDir;
// use xmltree::{Element, XMLNode};

use fs_err as fs;
use xmltree::Element;
// // Assuming these are defined elsewhere
// use crate::utils::{UnixLex, add_xml_child, indent_xml_tree, logged_run, unlink};
// use crate::base::{Menu, MenuItem, menuitem_defaults};

use std::{fs::File, path::PathBuf};

use rattler_conda_types::Platform;

use crate::{schema, MenuMode};
use crate::{schema::Linux, MenuInstError};

pub struct LinuxMenu {
    item: Linux,
    mode: MenuMode,
    directories: Directories,
    directory_entry_location: PathBuf,
}

pub struct Directories {
    config_directory: PathBuf,
    data_directory: PathBuf,
    system_menu_config_location: PathBuf,
    desktop_entries_location: PathBuf,
    menu_config_location: PathBuf,
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
            let home_dir = dirs::home_dir().unwrap_or("/".into());
            (
                std::env::var("XDG_CONFIG_HOME")
                    .map_or_else(|_| home_dir.join(".config"), PathBuf::from),
                std::env::var("XDG_DATA_HOME")
                    .map_or_else(|_| home_dir.join(".local/share"), PathBuf::from),
            )
        };

        let menu_config_location = if mode == MenuMode::System {
            system_config_directory
                .join("menus")
                .join("applications.menu")
        } else {
            config_directory.join("menus").join("applications.menu")
        };

        Directories {
            config_directory: config_directory.clone(),
            data_directory: data_directory.clone(),
            system_menu_config_location: system_config_directory
                .join("menus")
                .join("applications.menu"),
            menu_config_location,
            desktop_entries_location: data_directory.join("applications"),
        }
    }

    /// Create the directories if they don't exist.
    pub fn ensure_directories_exist(&self) -> Result<(), std::io::Error> {
        let paths = vec![
            self.config_directory.join("menus"),
            self.data_directory.join("desktop-directories"),
            self.data_directory.join("applications"),
        ];
        for path in paths {
            tracing::info!("Ensuring path {:?} exists", path);
            fs_err::create_dir_all(path)?;
        }

        Ok(())
    }
}

const DIRECTORY_ENTRY_TEMPLATE: &str = r#"[Desktop Entry]
Type=Directory
Encoding=UTF-8
Name={NAME}"#;

/// This is the "top-level" Linux Menu Item. It creates a new menu item in the system menu.
/// This is basically a category in the system menu. The file is usually located at
/// `/etc/xdg/menus/applications.menu` or `~/.config/menus/applications.menu`.
///
/// We prefer to edit the user menu file and use the `MergeFile` tag to include the system menu file.
///
/// It edits a menu file that looks something like this:
/// ```xml
/// <!DOCTYPE Menu PUBLIC "-//freedesktop//DTD Menu 1.0//EN"
/// "http://standards.freedesktop.org/menu-spec/menu-1.0.dtd">
/// <Menu>
///   <Name>Applications</Name>
///
///   <!-- Accessories submenu -->
///   <Menu>
///     <Name>Accessories</Name>
///     <Directory>Utility.directory</Directory>
///     <Include>
///       <And>
///         <Category>Utility</Category>
///         <Not>
///           <Category>System</Category>
///         </Not>
///       </And>
///     </Include>
///   </Menu> <!-- End Accessories -->
/// </Menu>
/// ```
///
/// The `Directory` tag is a reference to a `.directory` file that contains the name of the category.
/// The `Include` tag is a reference to the category itself.
/// The `Name` field is the name of the category.

/// The `.directory` file looks like this:
///
/// ```ini
/// [Desktop Entry]
/// Type=Directory
/// Encoding=UTF-8
/// Name=MyApp
/// ```
///
/// The actual `.desktop` files are located in `~/.local/share/applications` or `/usr/share/applications` and are created by the `LinuxMenuItem` struct.
impl LinuxMenu {
    fn new(item: Linux, mode: MenuMode) -> Self {
        let directories = Directories::new(mode);
        let directory_entry_location = directories
            .data_directory
            .join("desktop-directories")
            .join(format!("{}.directory", item.base.name));

        LinuxMenu {
            item,
            mode,
            directories,
            directory_entry_location,
        }
    }

    fn location(&self) -> PathBuf {
        // TODO: The Python implementation uses one more variable
        let filename = format!("{}.desktop", self.item.base.name);
        self.directories.desktop_entries_location.join(filename)
    }

    /// Create the parent menu category and the `.directory` file.
    pub fn create(&self) -> Result<(), MenuInstError> {
        self.directories.ensure_directories_exist()?;
        self.write_directory_entry()?;
        if self.is_valid_menu_file()? && self.has_this_menu()? {
            return Ok(());
        }
        self.ensure_menu_file()?;
        self.add_this_menu()?;
        Ok(())
    }

    /// Create a backup copy of the existing menu file and create a new one if it doesn't exist.
    fn ensure_menu_file(&self) -> Result<(), MenuInstError> {
        if self.directories.menu_config_location.exists()
            && !self.directories.menu_config_location.is_file()
        {
            return Err(MenuInstError::InstallError(format!(
                "Menu config location {:?} is not a file!",
                self.directories.menu_config_location
            )));
        }
        if self.directories.menu_config_location.is_file() {
            // Backup the existing menu file with a timestamp
            let timestamp = SystemTime::now();
            let backup_menu_file = format!(
                "{}.{}",
                self.directories.menu_config_location.display(),
                timestamp.duration_since(UNIX_EPOCH).unwrap().as_secs()
            );
            fs::copy(&self.directories.menu_config_location, backup_menu_file)?;

            if !self.is_valid_menu_file()? {
                fs::remove_file(&self.directories.menu_config_location)?;
            }
        } else {
            self.new_menu_file()?;
        }

        Ok(())
    }

    fn new_menu_file(&self) -> Result<(), MenuInstError> {
        let mut doc = Element::new("Menu");
        let mut name_node = Element::new("Name");
        name_node.children = vec![xmltree::XMLNode::Text("Applications".to_string())];
        doc.children.push(xmltree::XMLNode::Element(name_node));

        if self.mode == MenuMode::User {
            let mut merge_file = Element::new("MergeFile");
            merge_file
                .attributes
                .insert("type".to_string(), "parent".to_string());
            merge_file.children = vec![xmltree::XMLNode::Text(
                self.directories
                    .system_menu_config_location
                    .display()
                    .to_string(),
            )];
            doc.children.push(xmltree::XMLNode::Element(merge_file));
        }

        self.write_menu_file(&doc)
    }

    fn write_directory_entry(&self) -> Result<(), MenuInstError> {
        let content = DIRECTORY_ENTRY_TEMPLATE.replace("{NAME}", &self.item.base.name);
        fs::write(&self.directory_entry_location, content)?;
        Ok(())
    }

    fn get_xml_tree(&self) -> Result<Element, MenuInstError> {
        println!("Reading {:?}", self.directories.menu_config_location);
        let contents = fs::read_to_string(&self.directories.menu_config_location)?;
        Ok(Element::parse(contents.as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?)
    }

    fn has_this_menu(&self) -> Result<bool, MenuInstError> {
        let doc = self.get_xml_tree()?;
        Ok(doc.children.iter().any(|child| {
            if let xmltree::XMLNode::Element(element) = child {
                if element.name == "Menu" {
                    if let Some(name_element) = element.get_child("Name") {
                        match name_element.get_text() {
                            Some(name) => return name == self.item.base.name,
                            None => return false,
                        }
                    }
                }
            }
            false
        }))
    }

    fn is_valid_menu_file(&self) -> Result<bool, MenuInstError> {
        if !self.directories.menu_config_location.exists() {
            return Ok(false);
        }
        let doc = self.get_xml_tree()?;
        if doc.name != "Menu" {
            tracing::info!(
                "Menu file is not valid: {:?}",
                self.directories.menu_config_location
            );
        }
        Ok(doc.name == "Menu")
    }

    fn remove_this_menu(&self) -> Result<(), MenuInstError> {
        let mut doc = self.get_xml_tree()?;
        doc.children.retain(|child| {
            if let xmltree::XMLNode::Element(element) = child {
                if element.name == "Menu" {
                    if let Some(name_element) = element.get_child("Name") {
                        match name_element.get_text() {
                            Some(name) => return name != self.item.base.name,
                            None => return true,
                        }
                    }
                }
            }
            true
        });

        self.write_menu_file(&doc)?;

        Ok(())
    }

    fn write_menu_file(&self, doc: &Element) -> Result<(), MenuInstError> {
        let mut file = File::create(&self.directories.menu_config_location)?;
        writeln!(
            file,
            r#"<!DOCTYPE Menu PUBLIC "-//freedesktop//DTD Menu 1.0//EN""#
        )?;
        writeln!(
            file,
            r#" "http://standards.freedesktop.org/menu-spec/menu-1.0.dtd">"#
        )?;
        let emitter_config = xmltree::EmitterConfig {
            perform_indent: true,
            perform_escaping: true,
            // Do not add the XML declaration to the file (important)
            write_document_declaration: false,
            ..Default::default()
        };
        doc.write_with_config(&mut file, emitter_config)?;
        writeln!(file)?;
        Ok(())
    }

    fn add_this_menu(&self) -> Result<Element, MenuInstError> {
        println!("Adding {:?} to menu file", self.item.base.name);
        // Add the menu to the system menu config file using xmltree
        let mut doc = self.get_xml_tree()?;

        let mut menu_element = Element::new("Menu");
        let mut name_node = Element::new("Name");
        name_node.children = vec![xmltree::XMLNode::Text(self.item.base.name.clone())];
        menu_element
            .children
            .push(xmltree::XMLNode::Element(name_node));

        let mut directory_node = Element::new("Directory");
        directory_node.children = vec![xmltree::XMLNode::Text(format!(
            "{}.directory",
            self.item.base.name
        ))];
        menu_element
            .children
            .push(xmltree::XMLNode::Element(directory_node));

        let mut include_element = Element::new("Include");
        let mut category_node = Element::new("Category");
        category_node.children = vec![xmltree::XMLNode::Text(self.item.base.name.clone())];
        include_element
            .children
            .push(xmltree::XMLNode::Element(category_node));
        menu_element
            .children
            .push(xmltree::XMLNode::Element(include_element));

        doc.children.push(xmltree::XMLNode::Element(menu_element));

        self.write_menu_file(&doc)?;

        Ok(doc)
    }
}

struct LinuxMenuItem {
    item: schema::Linux,
    location: PathBuf,
    directories: Directories,
}

impl LinuxMenuItem {
    pub fn new(menu: schema::Linux, menu_mode: MenuMode) -> Self {
        let directories = Directories::new(menu_mode);

        // let menu_prefix = slug::slugify(&self.menu.item.base.name);
        // let name_slug = slug::slugify(&self.name);
        // let filename = format!("{}_{}.desktop", menu_prefix, name_slug);

        let location = directories
            .desktop_entries_location
            .join(format!("{}.desktop", menu.base.name));

        LinuxMenuItem {
            item: menu,
            location,
            directories,
        }
    }

    pub fn create(&self) -> Result<Vec<PathBuf>, MenuInstError> {
        tracing::debug!("Creating {:?}", self.location);
        self.write_desktop_file()?;
        self.maybe_register_mime_types(true)?;
        self.update_desktop_database()?;
        Ok(self.paths())
    }

    // pub fn remove(&self) -> Result<Vec<PathBuf>, MenuInstError> {
    //     let paths = self.paths();
    //     self.maybe_register_mime_types(false)?;
    //     for path in &paths {
    //         tracing::debug!("Removing {:?}", path);
    //         if let Err(e) = std::fs::remove_file(path) {
    //             if e.kind() != std::io::ErrorKind::NotFound {
    //                 return Err(MenuInstError::RemoveError(e));
    //             }
    //         }
    //     }
    //     self.update_desktop_database()?;
    //     Ok(paths)
    // }

    fn update_desktop_database(&self) -> Result<(), MenuInstError> {
        if let Ok(exe) = which::which("update-desktop-database") {
            Command::new(exe)
                .arg(self.directories.desktop_entries_location.to_str().unwrap())
                .output()?;
        }
        Ok(())
    }

    fn command(&self) -> String {
        let mut parts = Vec::new();
        // TODO: Implement precommand logic if needed
        // TODO: Implement activation logic if needed
        parts.push(shell_words::join(&self.item.base.command));
        format!("bash -c {}", shell_words::quote(&parts.join(" && ")))
    }

    fn write_desktop_file(&self) -> Result<(), MenuInstError> {
        println!("Writing {:?}", self.location);
        if self.location.exists() {
            tracing::warn!("Overwriting existing file at {:?}.", self.location);
        }

        let mut lines = vec![
            "[Desktop Entry]".to_string(),
            "Type=Application".to_string(),
            "Encoding=UTF-8".to_string(),
            format!("Name={}", self.item.base.name),
            format!("Exec={}", self.command()),
            format!(
                "Terminal={}",
                self.item
                    .base
                    .terminal
                    .unwrap_or(false)
                    .to_string()
                    .to_lowercase()
            ),
        ];

        if let Some(icon) = &self.item.base.icon {
            lines.push(format!("Icon={icon}"));
        }

        if !self.item.base.description.is_empty() {
            lines.push(format!("Comment={}", self.item.base.description));
        }

        if let Some(working_dir) = &self.item.base.working_dir {
            std::fs::create_dir_all(working_dir)?;
            lines.push(format!("Path={}", working_dir));
        }

        // TODO: Add additional keys from menuitem_defaults if needed
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

        let mut file = File::create(&self.location)?;
        for line in lines {
            writeln!(file, "{}", line)?;
        }

        Ok(())
    }

    fn maybe_register_mime_types(&self, register: bool) -> Result<(), MenuInstError> {
        if !self.item.mime_type.is_empty() {
            self.register_mime_types(register)?;
        }
        Ok(())
    }

    fn register_mime_types(&self, register: bool) -> Result<(), MenuInstError> {
        // TODO: Implement mime type registration logic
        Ok(())
    }

    fn paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.location.clone()];
        // TODO: Add paths for registered mime types if needed
        paths
    }
}

pub fn install_menu_item(item: Linux, menu_mode: MenuMode) -> Result<(), MenuInstError> {
    let menu = LinuxMenu::new(item.clone(), menu_mode);
    menu.create()?;

    let menu_item = LinuxMenuItem::new(item, menu_mode);
    menu_item.create()?;
    println!("{:?}", menu.location());
    println!("{:?}", menu.directories.config_directory);
    Ok(())
}
