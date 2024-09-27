use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

mod knownfolders;
mod registry;

struct Directories {
    start_menu_location: PathBuf,
    quick_launch_location: PathBuf,
    desktop_location: PathBuf,
}

impl Directories {
    pub fn create() -> Directories {
        let start_menu_location = dirs::start_menu_dir().unwrap();
        let quick_launch_location = dirs::quick_launch_dir().unwrap();
        let desktop_location = dirs::desktop_dir().unwrap();

        Directories {
            start_menu_location,
            quick_launch_location,
            desktop_location,
        }
    }
}

pub struct WindowsMenu {
    name: String,
    mode: String,
    prefix: PathBuf,
    base_prefix: PathBuf,
}
