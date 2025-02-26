//! This should take a `serde_json` file, render it with all variables and then load it as a `MenuInst` struct

use rattler_conda_types::Platform;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
#[serde(transparent)]
pub struct PlaceholderString(pub String);

impl PlaceholderString {
    pub fn resolve(&self, placeholder: impl AsRef<HashMap<String, String>>) -> String {
        replace_placeholders(self.0.clone(), placeholder.as_ref())
    }
}

pub struct BaseMenuItemPlaceholders {
    placeholders: HashMap<String, String>,
}

impl BaseMenuItemPlaceholders {
    pub fn new(base_prefix: &Path, prefix: &Path, platform: Platform) -> Self {
        let dist_name = |p: &Path| {
            p.file_name()
                .map_or_else(|| "empty".to_string(), |s| s.to_string_lossy().to_string())
        };

        let (python, base_python) = if platform.is_windows() {
            (prefix.join("python.exe"), base_prefix.join("python.exe"))
        } else {
            (prefix.join("bin/python"), base_prefix.join("bin/python"))
        };

        let mut vars = HashMap::from([
            ("BASE_PREFIX", base_prefix.to_path_buf()),
            ("PREFIX", prefix.to_path_buf()),
            ("PYTHON", python),
            ("BASE_PYTHON", base_python),
            ("MENU_DIR", prefix.join("Menu")),
            ("HOME", dirs::home_dir().unwrap_or_default()),
        ]);

        if platform.is_windows() {
            vars.insert("BIN_DIR", prefix.join("Library/bin"));
            vars.insert("SCRIPTS_DIR", prefix.join("Scripts"));
            vars.insert("BASE_PYTHONW", base_prefix.join("pythonw.exe"));
            vars.insert("PYTHONW", prefix.join("pythonw.exe"));
        } else {
            vars.insert("BIN_DIR", prefix.join("bin"));
        }

        if platform.is_osx() {
            vars.insert("PYTHONAPP", prefix.join("python.app/Contents/MacOS/python"));
        }

        let mut vars: HashMap<String, String> = vars
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string_lossy().to_string()))
            .collect();

        let icon_ext = if platform.is_windows() {
            "ico"
        } else if platform.is_osx() {
            "icns"
        } else {
            "png"
        };
        vars.insert("ICON_EXT".to_string(), icon_ext.to_string());

        vars.insert("DISTRIBUTION_NAME".to_string(), dist_name(prefix));
        vars.insert("ENV_NAME".to_string(), dist_name(prefix));

        BaseMenuItemPlaceholders { placeholders: vars }
    }

    /// Insert the menu item location into the placeholders
    ///
    /// - On Linux, this is the path to the `.desktop` file
    /// - On Windows, this is the path to the start menu `.lnk` file
    /// - On macOS, this is the path to the `.app` bundle
    pub fn refine(&self, menu_item_location: &Path) -> MenuItemPlaceholders {
        let mut vars = self.placeholders.clone();
        vars.insert(
            "MENU_ITEM_LOCATION".to_string(),
            menu_item_location.to_string_lossy().to_string(),
        );
        MenuItemPlaceholders { placeholders: vars }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MenuItemPlaceholders {
    placeholders: HashMap<String, String>,
}

impl AsRef<HashMap<String, String>> for MenuItemPlaceholders {
    fn as_ref(&self) -> &HashMap<String, String> {
        &self.placeholders
    }
}

impl AsRef<HashMap<String, String>> for BaseMenuItemPlaceholders {
    fn as_ref(&self) -> &HashMap<String, String> {
        &self.placeholders
    }
}

/// Replace placeholders in a string with values from a hashmap
/// This only replaces placeholders in the form of {{ key }} (note: while this looks like a Jinja template, it is not).
fn replace_placeholders(mut text: String, replacements: &HashMap<String, String>) -> String {
    for (key, value) in replacements {
        let placeholder = format!("{{{{ {key} }}}}");
        text = text.replace(&placeholder, value);
    }
    text
}
