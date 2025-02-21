//! Define types that can be serialized into a `PrefixRecord` to track
//! menu entries installed into the system.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Menu mode that was used to install the menu entries
#[derive(Default, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MenuMode {
    /// System-wide installation
    System,

    /// User installation
    #[default]
    User,
}

/// Tracker for menu entries installed into the system
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Tracker {
    /// Linux tracker
    Linux(LinuxTracker),
    /// Windows tracker
    Windows(WindowsTracker),
    /// macOS tracker
    MacOs(MacOsTracker),
}

/// Registered MIME file on the system
#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinuxRegisteredMimeFile {
    /// The application that was registered
    pub application: String,
    /// Path to use when calling `update-mime-database`
    pub database_path: PathBuf,
    /// The location of the config file that was edited
    pub config_file: PathBuf,
    /// The MIME types that were associated to the application
    pub mime_types: Vec<String>,
}

/// Tracker for Linux installations
#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinuxTracker {
    /// The menu mode that was used to install the menu entries
    pub install_mode: MenuMode,

    /// List of desktop files that were installed
    pub paths: Vec<PathBuf>,

    /// MIME types that were installed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_types: Option<LinuxRegisteredMimeFile>,

    /// MIME type glob files that were registered on the system
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub registered_mime_files: Vec<PathBuf>,
}

/// File extension that was installed
#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WindowsFileExtension {
    /// The file extension that was installed
    pub extension: String,
    /// The identifier of the file extension
    pub identifier: String,
}

/// URL protocol that was installed
#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WindowsUrlProtocol {
    /// The URL protocol that was installed
    pub protocol: String,
    /// The identifier of the URL protocol
    pub identifier: String,
}

/// Terminal profile that was installed
#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WindowsTerminalProfile {
    /// The name of the terminal profile
    pub configuration_file: PathBuf,
    /// The identifier of the terminal profile
    pub identifier: String,
}

/// Tracker for Windows installations
#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WindowsTracker {
    /// The menu mode that was used to install the menu entries
    pub menu_mode: MenuMode,

    /// List of shortcuts that were installed
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shortcuts: Vec<PathBuf>,

    /// List of file extensions that were installed
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_extensions: Vec<WindowsFileExtension>,

    /// List of URL protocols that were installed
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub url_protocols: Vec<WindowsUrlProtocol>,

    /// List of terminal profiles that were installed
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terminal_profiles: Vec<WindowsTerminalProfile>,
}

impl WindowsTracker {
    /// Create a new Windows tracker
    pub fn new(menu_mode: MenuMode) -> Self {
        Self {
            menu_mode,
            shortcuts: Vec::new(),
            file_extensions: Vec::new(),
            url_protocols: Vec::new(),
            terminal_profiles: Vec::new(),
        }
    }
}

/// Tracker for macOS installations
#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MacOsTracker {
    /// The app folder that was installed, e.g. ~/Applications/foobar.app
    pub app_folder: PathBuf,
    /// Argument that was used to call `lsregister` and that we need to
    /// call to unregister the app
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lsregister: Option<PathBuf>,
}
