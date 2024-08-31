use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Local;
use log::{debug, warn};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use xmltree::{Element, XMLNode};

// Assuming these are defined elsewhere
use crate::utils::{UnixLex, add_xml_child, indent_xml_tree, logged_run, unlink};
use crate::base::{Menu, MenuItem, menuitem_defaults};

#[derive(Debug)]
pub struct LinuxMenu {
    name: String,
    mode: String,
    config_directory: PathBuf,
    data_directory: PathBuf,
    system_menu_config_location: PathBuf,
    menu_config_location: PathBuf,
    directory_entry_location: PathBuf,
    desktop_entries_location: PathBuf,
}

impl LinuxMenu {
    pub fn new(name: String, mode: String) -> Self {
        let system_config_directory = PathBuf::from("/etc/xdg/");
        let system_data_directory = PathBuf::from("/usr/share");

        let (config_directory, data_directory) = if mode == "system" {
            (system_config_directory.clone(), system_data_directory.clone())
        } else {
            (
                PathBuf::from(env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| "~/.config".to_string())),
                PathBuf::from(env::var("XDG_DATA_HOME").unwrap_or_else(|_| "~/.local/share".to_string())),
            )
        };

        LinuxMenu {
            name,
            mode,
            config_directory: config_directory.clone(),
            data_directory: data_directory.clone(),
            system_menu_config_location: system_config_directory.join("menus").join("applications.menu"),
            menu_config_location: config_directory.join("menus").join("applications.menu"),
            directory_entry_location: data_directory.join("desktop-directories").join(format!("{}.directory", Self::render(&name, true))),
            desktop_entries_location: data_directory.join("applications"),
        }
    }

    pub fn create(&self) -> Vec<PathBuf> {
        self.ensure_directories_exist();
        let path = self.write_directory_entry();
        if self.is_valid_menu_file() && self.has_this_menu() {
            return vec![path];
        }
        self.ensure_menu_file();
        self.add_this_menu();
        vec![path]
    }

    pub fn remove(&self) -> Vec<PathBuf> {
        for entry in fs::read_dir(&self.desktop_entries_location).unwrap() {
            let entry = entry.unwrap();
            let file_name = entry.file_name();
            if file_name.to_str().unwrap().starts_with(&format!("{}_", Self::render(&self.name, true))) {
                // found one shortcut, so don't remove the name from menu
                return vec![self.directory_entry_location.clone()];
            }
        }
        unlink(&self.directory_entry_location, true);
        self.remove_this_menu();
        vec![self.directory_entry_location.clone()]
    }

    fn ensure_directories_exist(&self) {
        let paths = vec![
            self.config_directory.join("menus"),
            self.data_directory.join("desktop-directories"),
            self.data_directory.join("applications"),
        ];
        for path in paths {
            debug!("Ensuring path {:?} exists", path);
            fs::create_dir_all(path).unwrap();
        }
    }

    fn write_directory_entry(&self) -> PathBuf {
        let content = format!(
            "[Desktop Entry]\nType=Directory\nEncoding=UTF-8\nName={}",
            Self::render(&self.name, false)
        );
        debug!("Writing directory entry at {:?}", self.directory_entry_location);
        fs::write(&self.directory_entry_location, content).unwrap();
        self.directory_entry_location.clone()
    }

    fn remove_this_menu(&self) {
        debug!("Editing {:?} to remove {} config", self.menu_config_location, Self::render(&self.name, false));
        let mut doc = Element::parse(fs::read_to_string(&self.menu_config_location).unwrap().as_bytes()).unwrap();
        doc.children.retain(|child| {
            if let XMLNode::Element(element) = child {
                if element.name == "Menu" {
                    if let Some(name_element) = element.get_child("Name") {
                        return name_element.get_text() != Some(Self::render(&self.name, false));
                    }
                }
            }
            true
        });
        self.write_menu_file(&doc);
    }

    fn has_this_menu(&self) -> bool {
        let doc = Element::parse(fs::read_to_string(&self.menu_config_location).unwrap().as_bytes()).unwrap();
        doc.children.iter().any(|child| {
            if let XMLNode::Element(element) = child {
                if element.name == "Menu" {
                    if let Some(name_element) = element.get_child("Name") {
                        return name_element.get_text() == Some(Self::render(&self.name, false));
                    }
                }
            }
            false
        })
    }

    fn add_this_menu(&self) {
        debug!("Editing {:?} to add {} config", self.menu_config_location, Self::render(&self.name, false));
        let mut doc = Element::parse(fs::read_to_string(&self.menu_config_location).unwrap().as_bytes()).unwrap();
        let mut menu_element = Element::new("Menu");
        add_xml_child(&mut menu_element, "Name", Self::render(&self.name, false));
        add_xml_child(&mut menu_element, "Directory", format!("{}.directory", Self::render(&self.name, true)));
        let mut inc_element = Element::new("Include");
        add_xml_child(&mut inc_element, "Category", Self::render(&self.name, false));
        menu_element.children.push(XMLNode::Element(inc_element));
        doc.children.push(XMLNode::Element(menu_element));
        self.write_menu_file(&doc);
    }

    fn is_valid_menu_file(&self) -> bool {
        if let Ok(content) = fs::read_to_string(&self.menu_config_location) {
            if let Ok(doc) = Element::parse(content.as_bytes()) {
                return doc.name == "Menu";
            }
        }
        false
    }

    fn write_menu_file(&self, doc: &Element) {
        debug!("Writing {:?}", self.menu_config_location);
        indent_xml_tree(doc);
        let mut file = File::create(&self.menu_config_location).unwrap();
        writeln!(file, r#"<!DOCTYPE Menu PUBLIC "-//freedesktop//DTD Menu 1.0//EN""#).unwrap();
        writeln!(file, r#" "http://standards.freedesktop.org/menu-spec/menu-1.0.dtd">"#).unwrap();
        doc.write(&mut file).unwrap();
        writeln!(file).unwrap();
    }

    fn ensure_menu_file(&self) {
        if self.menu_config_location.exists() && !self.menu_config_location.is_file() {
            panic!("Menu config location {:?} is not a file!", self.menu_config_location);
        }

        if self.menu_config_location.is_file() {
            let cur_time = Local::now().format("%Y-%m-%d_%Hh%Mm%S").to_string();
            let backup_menu_file = format!("{}.{}", self.menu_config_location.display(), cur_time);
            fs::copy(&self.menu_config_location, backup_menu_file).unwrap();

            if !self.is_valid_menu_file() {
                fs::remove_file(&self.menu_config_location).unwrap();
            }
        } else {
            self.new_menu_file();
        }
    }

    fn new_menu_file(&self) {
        debug!("Creating {:?}", self.menu_config_location);
        let mut content = String::from("<Menu><Name>Applications</Name>");
        if self.mode == "user" {
            content.push_str(&format!(r#"<MergeFile type="parent">{}</MergeFile>"#, self.system_menu_config_location.display()));
        }
        content.push_str("</Menu>\n");
        fs::write(&self.menu_config_location, content).unwrap();
    }

    fn render(name: &str, slug: bool) -> String {
        // Implement rendering logic here
        if slug {
            name.to_lowercase().replace(" ", "-")
        } else {
            name.to_string()
        }
    }
}

// Implement LinuxMenuItem struct and its methods similarly...
