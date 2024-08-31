use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use log::{debug, warn};
use serde_json::Value;
use tempfile::NamedTempFile;
use winapi::um::winuser::SHChangeNotify;

use crate::utils::{WinLex, logged_run, unlink};
use crate::base::{Menu, MenuItem};
use crate::win_utils::knownfolders::{folder_path, windows_terminal_settings_files};
use crate::win_utils::registry::{
    notify_shell_changes, register_file_extension, register_url_protocol,
    unregister_file_extension, unregister_url_protocol,
};

pub struct WindowsMenu {
    name: String,
    mode: String,
    prefix: PathBuf,
    base_prefix: PathBuf,
}

impl WindowsMenu {
    pub fn new(name: String, mode: String, prefix: PathBuf, base_prefix: PathBuf) -> Self {
        WindowsMenu { name, mode, prefix, base_prefix }
    }

    pub fn create(&self) -> Vec<PathBuf> {
        debug!("Creating {:?}", self.start_menu_location());
        fs::create_dir_all(&self.start_menu_location()).unwrap();
        if let Some(quick_launch) = self.quick_launch_location() {
            fs::create_dir_all(quick_launch).unwrap();
        }
        if let Some(desktop) = self.desktop_location() {
            fs::create_dir_all(desktop).unwrap();
        }
        vec![self.start_menu_location()]
    }

    pub fn remove(&self) -> Vec<PathBuf> {
        let menu_location = self.start_menu_location();
        if menu_location.exists() {
            if menu_location.read_dir().unwrap().next().is_none() {
                debug!("Removing {:?}", menu_location);
                fs::remove_dir_all(menu_location).unwrap();
            }
        }
        vec![menu_location]
    }

    pub fn start_menu_location(&self) -> PathBuf {
        PathBuf::from(folder_path(&self.mode, false, "start")).join(self.render(&self.name))
    }

    pub fn quick_launch_location(&self) -> Option<PathBuf> {
        if self.mode == "system" {
            warn!("Quick launch menus are not available for system level installs");
            None
        } else {
            Some(PathBuf::from(folder_path(&self.mode, false, "quicklaunch")))
        }
    }

    pub fn desktop_location(&self) -> Option<PathBuf> {
        Some(PathBuf::from(folder_path(&self.mode, false, "desktop")))
    }

    pub fn terminal_profile_locations(&self) -> Vec<PathBuf> {
        if self.mode == "system" {
            warn!("Terminal profiles are not available for system level installs");
            vec![]
        } else {
            windows_terminal_settings_files(&self.mode)
        }
    }

    pub fn placeholders(&self) -> HashMap<String, String> {
        let mut placeholders = HashMap::new();
        placeholders.insert("SCRIPTS_DIR".to_string(), self.prefix.join("Scripts").to_str().unwrap().to_string());
        placeholders.insert("PYTHON".to_string(), self.prefix.join("python.exe").to_str().unwrap().to_string());
        placeholders.insert("PYTHONW".to_string(), self.prefix.join("pythonw.exe").to_str().unwrap().to_string());
        placeholders.insert("BASE_PYTHON".to_string(), self.base_prefix.join("python.exe").to_str().unwrap().to_string());
        placeholders.insert("BASE_PYTHONW".to_string(), self.base_prefix.join("pythonw.exe").to_str().unwrap().to_string());
        placeholders.insert("BIN_DIR".to_string(), self.prefix.join("Library").join("bin").to_str().unwrap().to_string());
        placeholders.insert("SP_DIR".to_string(), self.site_packages().to_str().unwrap().to_string());
        placeholders.insert("ICON_EXT".to_string(), "ico".to_string());
        placeholders
    }

    pub fn render(&self, value: &str, slug: bool, extra: Option<&HashMap<String, String>>) -> String {
        // Implement rendering logic here
        let rendered = value.to_string(); // Placeholder for actual rendering
        if rendered.contains('/') && !rendered.starts_with('/') {
            rendered.replace('/', "\\")
        } else {
            rendered
        }
    }

    fn site_packages(&self) -> PathBuf {
        self.prefix.join("Lib").join("site-packages")
    }

    fn paths(&self) -> Vec<PathBuf> {
        vec![self.start_menu_location()]
    }
}

pub struct WindowsMenuItem {
    menu: WindowsMenu,
    metadata: HashMap<String, Value>,
}

impl WindowsMenuItem {
    pub fn new(menu: WindowsMenu, metadata: HashMap<String, Value>) -> Self {
        WindowsMenuItem { menu, metadata }
    }

    pub fn location(&self) -> PathBuf {
        self.menu.start_menu_location().join(self.shortcut_filename())
    }

    pub fn create(&self) -> Vec<PathBuf> {
        self.precreate();
        let paths = self.paths();

        for path in &paths {
            if path.extension().unwrap_or_default() != "lnk" {
                continue;
            }

            let (target_path, arguments) = self.process_command();
            let working_dir = self.render_key("working_dir");
            let working_dir = if let Some(dir) = working_dir {
                fs::create_dir_all(std::path::Path::new(&std::env::var("USERPROFILE").unwrap()).join(dir)).unwrap();
                dir
            } else if std::env::var("USERPROFILE").is_ok() {
                "%USERPROFILE%".to_string()
            } else {
                "%HOMEDRIVE%%HOMEPATH%".to_string()
            };

            let icon = self.render_key("icon").unwrap_or_default();

            if path.exists() {
                warn!("Overwriting existing link at {:?}.", path);
            }
            // Implement create_shortcut function
            create_shortcut(
                &target_path,
                &self.shortcut_filename_without_ext(),
                path,
                &arguments.join(" "),
                &working_dir,
                &icon,
                0,
                &self.app_user_model_id(),
            );
        }

        for location in self.menu.terminal_profile_locations() {
            self.add_remove_windows_terminal_profile(&location, false);
        }
        let changed_extensions = self.register_file_extensions();
        let changed_protocols = self.register_url_protocols();
        if changed_extensions || changed_protocols {
            notify_shell_changes();
        }

        paths
    }

    pub fn remove(&self) -> Vec<PathBuf> {
        let changed_extensions = self.unregister_file_extensions();
        let changed_protocols = self.unregister_url_protocols();
        if changed_extensions || changed_protocols {
            notify_shell_changes();
        }

        for location in self.menu.terminal_profile_locations() {
            self.add_remove_windows_terminal_profile(&location, true);
        }

        let paths = self.paths();
        for path in &paths {
            debug!("Removing {:?}", path);
            unlink(path, true);
        }

        paths
    }

    fn paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.location()];
        let mut extra_dirs = vec![];
        if self.metadata.get("desktop").and_then(|v| v.as_bool()).unwrap_or(false) {
            if let Some(desktop) = self.menu.desktop_location() {
                extra_dirs.push(desktop);
            }
        }
        if self.metadata.get("quicklaunch").and_then(|v| v.as_bool()).unwrap_or(false) {
            if let Some(quick_launch) = self.menu.quick_launch_location() {
                extra_dirs.push(quick_launch);
            }
        }

        for dir in extra_dirs {
            paths.push(dir.join(self.shortcut_filename()));
        }

        if self.metadata.get("activate").and_then(|v| v.as_bool()).unwrap_or(false) {
            paths.push(self.path_for_script());
        }

        paths
    }

    fn shortcut_filename(&self) -> String {
        format!("{}.lnk", self.render_key("name").unwrap())
    }

    fn shortcut_filename_without_ext(&self) -> String {
        self.render_key("name").unwrap()
    }

    fn path_for_script(&self) -> PathBuf {
        PathBuf::from(self.menu.placeholders().get("MENU_DIR").unwrap())
            .join(format!("{}.bat", self.render_key("name").unwrap()))
    }

    fn precreate(&self) {
        if let Some(precreate_code) = self.render_key("precreate") {
            let mut tmp = NamedTempFile::new().unwrap();
            writeln!(tmp, "{}", precreate_code).unwrap();
            let system32 = PathBuf::from(std::env::var("SystemRoot").unwrap_or("C:\\Windows".to_string())).join("system32");
            let cmd = [
                system32.join("WindowsPowerShell").join("v1.0").join("powershell.exe").to_str().unwrap(),
                &format!("\"start '{}' -WindowStyle hidden\"", tmp.path().to_str().unwrap()),
            ];
            logged_run(&cmd, true).unwrap();
        }
    }

    fn command(&self) -> String {
        let mut lines = vec![
            "@ECHO OFF".to_string(),
            ":: Script generated by conda/menuinst".to_string(),
        ];
        if let Some(precommand) = self.render_key("precommand") {
            lines.push(precommand);
        }
        if self.metadata.get("activate").and_then(|v| v.as_bool()).unwrap_or(false) {
            let conda_exe = &self.menu.prefix.join("Scripts").join("conda.exe");
            let activate = if self.menu.is_micromamba(conda_exe) { "shell activate" } else { "shell.cmd.exe activate" };
            let activator = format!(r#"{} {} "{}""#, conda_exe.to_str().unwrap(), activate, self.menu.prefix.to_str().unwrap());
            lines.extend_from_slice(&[
                "@SETLOCAL ENABLEDELAYEDEXPANSION".to_string(),
                format!(r#"@FOR /F "usebackq tokens=*" %%i IN (`{}`) do set "ACTIVATOR=%%i""#, activator),
                "@CALL %ACTIVATOR%".to_string(),
                ":: This below is the user command".to_string(),
            ]);
        }

        lines.push(WinLex::quote_args(&self.render_key("command").unwrap()));

        lines.join("\r\n")
    }

    fn write_script(&self, script_path: Option<&Path>) -> PathBuf {
        let script_path = script_path.unwrap_or_else(|| self.path_for_script().as_path());
        fs::create_dir_all(script_path.parent().unwrap()).unwrap();
        fs::write(script_path, self.command()).unwrap();
        script_path.to_path_buf()
    }

    fn process_command(&self, with_arg1: bool) -> (String, Vec<String>) {
        if self.metadata.get("activate").and_then(|v| v.as_bool()).unwrap_or(false) {
            let script = self.write_script(None);
            if self.metadata.get("terminal").and_then(|v| v.as_bool()).unwrap_or(false) {
                let mut command = vec!["cmd".to_string(), "/D".to_string(), "/K".to_string(), format!("\"{}\"", script.to_str().unwrap())];
                if with_arg1 {
                    command.push("%1".to_string());
                }
                (command[0].clone(), command[1..].to_vec())
            } else {
                let system32 = PathBuf::from(std::env::var("SystemRoot").unwrap_or("C:\\Windows".to_string())).join("system32");
                let arg1 = if with_arg1 { "%1 " } else { "" };
                let command = vec![
                    format!("\"{}\"", system32.join("cmd.exe").to_str().unwrap()),
                    "/D".to_string(),
                    "/C".to_string(),
                    "START".to_string(),
                    "/MIN".to_string(),
                    "\"\"".to_string(),
                    format!("\"{}\"", system32.join("WindowsPowerShell").join("v1.0").join("powershell.exe").to_str().unwrap()),
                    "-WindowStyle".to_string(),
                    "hidden".to_string(),
                    format!("\"start '{}' {}-WindowStyle hidden\"", script.to_str().unwrap(), arg1),
                ];
                (command[0].clone(), command[1..].to_vec())
            }
        } else {
            let mut command = self.render_key("command").unwrap();
            if with_arg1 && !command.iter().any(|arg| arg.contains("%1")) {
                command.push("%1".to_string());
            }
            let quoted = WinLex::quote_args(&command);
            (quoted[0].clone(), quoted[1..].to_vec())
        }
    }

    // Implement other methods (add_remove_windows_terminal_profile, register_file_extensions, etc.) similarly...

    fn app_user_model_id(&self) -> String {
        self.render_key("app_user_model_id")
            .unwrap_or_else(|| format!("Menuinst.{}", self.render_key("name").unwrap().to_lowercase().replace('.', "")))
            .chars()
            .take(128)
            .collect()
    }

    fn render_key(&self, key: &str) -> Option<String> {
        // Implement rendering logic here
        self.metadata.get(key).and_then(|v| v.as_str().map(|s| s.to_string()))
    }
}

// Implement create_shortcut function
fn create_shortcut(target_path: &str, description: &str, shortcut_path: &Path, arguments: &str, working_dir: &str, icon_path: &str, icon_index: i32, app_id: &str) {
    // Implement Windows shortcut creation logic here
    // This might require using the Windows API or a third-party crate
}
