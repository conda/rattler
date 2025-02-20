use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::MenuMode;

use super::knownfolders::{Folder, Folders, UserHandle};

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct TerminalProfile {
    pub commandline: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "startingDirectory")]
    pub starting_directory: Option<String>,
}

#[derive(Default, Debug, Serialize, Deserialize)]
struct Profiles {
    list: Vec<TerminalProfile>,
    #[serde(flatten)]
    other: BTreeMap<String, Value>,
}

#[derive(Default, Debug, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    profiles: Option<Profiles>,
    #[serde(flatten)]
    other: BTreeMap<String, Value>,
}

#[derive(Debug, Error)]
pub enum TerminalUpdateError {
    #[error("Failed to read or write settings file")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse settings file")]
    Serde(#[from] serde_json::Error),
}

/// Adds or updates a Windows Terminal profile in the settings file at the specified location.
/// If a profile with the same name exists, it is overwritten.
///
/// # Arguments
/// * `location` - Path to the settings.json file
/// * `profile` - The `TerminalProfile` to add or update
///
/// # Errors
/// Returns a `TerminalUpdateError` if file operations or JSON serialization/deserialization fail.
///
/// # Examples
/// ```ignore
/// let profile = TerminalProfile {
///     commandline: "cmd.exe".to_string(),
///     name: "My Profile".to_string(),
///     icon: Some("icon.png".to_string()),
///     starting_directory: None,
/// };
/// add_windows_terminal_profile(Path::new("settings.json"), &profile)?;
/// ```
pub fn add_windows_terminal_profile(
    location: &Path,
    profile: &TerminalProfile,
) -> Result<(), TerminalUpdateError> {
    // Early return if no parent directory exists
    if !location.parent().is_some_and(Path::exists) {
        tracing::warn!("Parent directory does not exist for {:?}", location);
        return Ok(());
    }

    // Read existing settings or create new
    let mut settings: Settings = if location.exists() {
        let content = fs::read_to_string(location)?;
        serde_json::from_str(&content)?
    } else {
        Settings::default()
    };

    let name = profile.name.clone();

    // Ensure profiles structure exists
    if settings.profiles.is_none() {
        settings.profiles = Some(Profiles::default());
    }

    let profiles = settings.profiles.as_mut().unwrap();

    // Update or append profile
    if let Some(index) = profiles.list.iter().position(|el| el.name == name) {
        tracing::warn!("Overwriting terminal profile for {}", name);
        profiles.list[index] = profile.clone();
    } else {
        profiles.list.push(profile.clone());
    }

    // Write back to file
    let json = serde_json::to_string_pretty(&settings)?;
    fs::write(location, json)?;

    Ok(())
}

/// Removes a profile with the specified name from the Windows Terminal settings file.
///
/// If the file or the named profile does not exist, this function does nothing and returns
/// successfully. The updated settings are written back to the file.
///
/// # Arguments
/// * `location` - Path to the `settings.json` file from which to remove the profile.
/// * `name` - The name of the profile to remove.
///
/// # Errors
/// Returns a `TerminalUpdateError` if:
/// - The file cannot be read or written (e.g., insufficient permissions).
/// - The existing settings file contains invalid JSON.
/// - Serialization to JSON fails.
///
/// # Examples
/// ```ignore
/// remove_terminal_profile(Path::new("settings.json"), "My Profile")?;
/// ```
pub fn remove_terminal_profile(location: &Path, name: &str) -> Result<(), TerminalUpdateError> {
    // Read existing settings or return early if no file exists
    let mut settings: Settings = if location.exists() {
        let content = fs::read_to_string(location)?;
        serde_json::from_str(&content)?
    } else {
        return Ok(());
    };

    // Ensure that profiles exist
    let Some(profiles) = settings.profiles.as_mut() else {
        return Ok(());
    };

    // Remove profile
    profiles.list.retain(|el| el.name != name);

    // Write back to file
    let json = serde_json::to_string_pretty(&settings)?;
    fs::write(location, json)?;

    Ok(())
}

/// Retrieves a list of potential Windows Terminal settings file locations for the current user.
///
/// This function searches for settings files in both packaged (Microsoft Store) and unpackaged
/// (e.g., Scoop, Chocolatey) installations of Windows Terminal, including stable and preview
/// versions. Returns an empty vector if not in `User` mode or if folder paths cannot be retrieved.
///
/// # Arguments
/// * `mode` - The `MenuMode` specifying the context; only `User` mode is supported.
/// * `folders` - A `Folders` instance providing access to known folder paths (e.g., `LocalAppData`).
///
/// # Returns
/// A vector of `PathBuf` objects representing the locations of `settings.json` files.
///
/// # Examples
/// ```ignore
/// let folders = Folders::new(); // Assuming Folders impl
/// let paths = windows_terminal_settings_files(MenuMode::User, &folders);
/// for path in paths {
///     println!("Found settings at: {:?}", path);
/// }
/// ```
pub fn windows_terminal_settings_files(mode: MenuMode, folders: &Folders) -> Vec<PathBuf> {
    if mode != MenuMode::User {
        return Vec::new();
    }

    // Assuming folder_path is a function you have that returns a String
    let Ok(localappdata) = folders.get_folder_path(Folder::LocalAppData, UserHandle::Current)
    else {
        return Vec::new();
    };

    let packages = localappdata.join("Packages");

    let mut profile_locations = Vec::new();

    if let Ok(entries) = fs::read_dir(&packages) {
        for entry in entries.filter_map(Result::ok) {
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();

            // Check for both stable and preview versions
            if file_name.starts_with("Microsoft.WindowsTerminal_")
                || file_name.starts_with("Microsoft.WindowsTerminalPreview_")
            {
                profile_locations.push(entry.path().join("LocalState").join("settings.json"));
            }
        }
    }

    // Unpackaged (Scoop, Chocolatey, etc.)
    let unpackaged_path = localappdata.join("Microsoft/Windows Terminal/settings.json");

    if unpackaged_path.parent().is_some_and(Path::exists) {
        profile_locations.push(unpackaged_path);
    }

    tracing::debug!(
        "Found Windows Terminal settings files: {:?}",
        profile_locations
    );
    profile_locations
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_json_snapshot;
    use tempfile::{NamedTempFile, TempDir};

    fn create_test_profile() -> TerminalProfile {
        TerminalProfile {
            commandline: "cmd.exe".to_string(),
            name: "Test Profile".to_string(),
            icon: Some("🚀".to_string()),
            starting_directory: Some("C:\\Users".to_string()),
        }
    }

    #[test]
    fn test_add_new_profile() {
        let temp_file = NamedTempFile::new().unwrap();
        let initial_json = r#"{
            "defaultProfile": "Windows.Terminal.Cmd",
            "theme": "dark",
            "profiles": {
                "defaults": {
                    "fontFace": "Cascadia Code"
                },
                "list": []
            }
        }"#;
        fs::write(temp_file.path(), initial_json).unwrap();

        let profile = create_test_profile();
        add_windows_terminal_profile(temp_file.path(), &profile).unwrap();

        let content = fs::read_to_string(temp_file.path()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_json_snapshot!(json, @r###"
        {
          "profiles": {
            "list": [
              {
                "commandline": "cmd.exe",
                "name": "Test Profile",
                "icon": "🚀",
                "startingDirectory": "C:\\Users"
              }
            ],
            "defaults": {
              "fontFace": "Cascadia Code"
            }
          },
          "defaultProfile": "Windows.Terminal.Cmd",
          "theme": "dark"
        }
        "###);
    }

    #[test]
    fn test_update_existing_profile() {
        let temp_file = NamedTempFile::new().unwrap();
        let initial_json = r#"{
            "profiles": {
                "list": [
                    {
                        "commandline": "old.exe",
                        "name": "Test Profile",
                        "icon": "⭐"
                    }
                ]
            }
        }"#;
        fs::write(temp_file.path(), initial_json).unwrap();

        let profile = create_test_profile();
        add_windows_terminal_profile(temp_file.path(), &profile).unwrap();

        let content = fs::read_to_string(temp_file.path()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_json_snapshot!(json, @r###"
        {
          "profiles": {
            "list": [
              {
                "commandline": "cmd.exe",
                "name": "Test Profile",
                "icon": "🚀",
                "startingDirectory": "C:\\Users"
              }
            ]
          }
        }
        "###);
    }

    #[test]
    fn test_remove_profile() {
        let temp_file = NamedTempFile::new().unwrap();
        let initial_json = r#"{
            "theme": "dark",
            "profiles": {
                "list": [
                    {
                        "commandline": "cmd.exe",
                        "name": "Test Profile",
                        "icon": "🚀"
                    },
                    {
                        "commandline": "powershell.exe",
                        "name": "PowerShell",
                        "icon": "⚡"
                    }
                ]
            }
        }"#;
        fs::write(temp_file.path(), initial_json).unwrap();

        remove_terminal_profile(temp_file.path(), "Test Profile").unwrap();

        let content = fs::read_to_string(temp_file.path()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_json_snapshot!(json, @r###"
        {
          "profiles": {
            "list": [
              {
                "commandline": "powershell.exe",
                "name": "PowerShell",
                "icon": "⚡"
              }
            ]
          },
          "theme": "dark"
        }
        "###);
    }

    #[test]
    fn test_add_profile_to_empty_file() {
        let temp_dir = TempDir::new().unwrap();
        let profile = create_test_profile();
        let temp_file = temp_dir.path().join("settings.json");
        add_windows_terminal_profile(&temp_file, &profile).unwrap();

        let content = fs::read_to_string(temp_file).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_json_snapshot!(json, @r###"
        {
          "profiles": {
            "list": [
              {
                "commandline": "cmd.exe",
                "name": "Test Profile",
                "icon": "🚀",
                "startingDirectory": "C:\\Users"
              }
            ]
          }
        }
        "###);
    }

    #[test]
    fn test_remove_profile_from_nonexistent_file() {
        let temp_dir = TempDir::new().unwrap();
        let result =
            remove_terminal_profile(&temp_dir.path().join("settings.json"), "Test Profile");
        assert!(result.is_ok());
    }

    #[test]
    fn test_preserve_unknown_fields() {
        let temp_file = NamedTempFile::new().unwrap();
        let initial_json = r#"{
            "customSetting": true,
            "profiles": {
                "customProfileSetting": "value",
                "list": []
            },
            "keybindings": [
                {
                    "command": "copy",
                    "keys": "ctrl+c"
                }
            ]
        }"#;
        fs::write(temp_file.path(), initial_json).unwrap();

        let profile = create_test_profile();
        add_windows_terminal_profile(temp_file.path(), &profile).unwrap();

        let content = fs::read_to_string(temp_file.path()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_json_snapshot!(json, @r###"
        {
          "profiles": {
            "list": [
              {
                "commandline": "cmd.exe",
                "name": "Test Profile",
                "icon": "🚀",
                "startingDirectory": "C:\\Users"
              }
            ],
            "customProfileSetting": "value"
          },
          "customSetting": true,
          "keybindings": [
            {
              "command": "copy",
              "keys": "ctrl+c"
            }
          ]
        }
        "###);
    }
}
