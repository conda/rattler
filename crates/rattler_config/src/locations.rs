//! Standard configuration file locations shared by rattler-based tools.
//!
//! Configuration comes from two layers:
//!
//! - the **shared** layer: files every rattler-based tool reads. They may
//!   only contain the keys shared by all tools ([`crate::config::CommonConfig`]);
//!   tool-specific keys in these files are ignored with a warning.
//! - the **tool** layer: the tool's own files, which accept the shared keys
//!   plus the tool-specific extension keys.
//!
//! The shared layer lives in the `rattler` directory:
//! `/etc/rattler/config.toml` (`C:\ProgramData\rattler\config.toml` on
//! Windows) and `$XDG_CONFIG_HOME/rattler/config.toml` (or the platform
//! equivalent reported by [`dirs::config_dir`]). `$RATTLER_HOME/config.toml`
//! is honored when the environment variable is set, but unlike the tool
//! layer there is no `~/.rattler` fallback: the shared layer is pure
//! configuration and does not warrant a home directory.
//!
//! Each tool has three conventional configuration locations, from lowest to
//! highest precedence:
//!
//! 1. a system-wide file: `/etc/<tool>/config.toml` (Linux/macOS) or
//!    `C:\ProgramData\<tool>\config.toml` (Windows),
//! 2. a file in the user configuration directory:
//!    `$XDG_CONFIG_HOME/<tool>/config.toml` (or the platform equivalent
//!    reported by [`dirs::config_dir`]),
//! 3. a file in the tool's home directory: `$<TOOL>_HOME/config.toml` if the
//!    environment variable is set (e.g. `PIXI_HOME`), otherwise
//!    `~/.<tool>/config.toml`.
//!
//! [`config_search_paths`] combines both layers for a tool, from lowest to
//! highest precedence: system shared, system tool, user shared, user tool.
//! The user always overrides the system, and within each level the
//! tool-specific file overrides the shared one.

use std::path::PathBuf;

/// The conventional file name of a configuration file.
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// The directory name of the shared configuration layer.
pub const SHARED_CONFIG_DIR: &str = "rattler";

/// The configuration layer a file belongs to, which determines the keys the
/// file may contain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigLayer {
    /// A file shared by all rattler-based tools; only the common keys are
    /// allowed.
    Shared,
    /// A tool's own file; common keys plus the tool's extension keys are
    /// allowed.
    Tool,
}

/// A candidate configuration file together with the layer it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigLocation {
    /// The path of the configuration file.
    pub path: PathBuf,
    /// The layer the file belongs to.
    pub layer: ConfigLayer,
}

/// The name of the environment variable pointing at a tool's home directory,
/// e.g. `PIXI_HOME` for `pixi` or `RATTLER_BUILD_HOME` for `rattler-build`.
fn home_env_var(tool: &str) -> String {
    format!("{}_HOME", tool.to_uppercase().replace('-', "_"))
}

/// The system-wide configuration file for a tool:
/// `/etc/<tool>/config.toml`, or `C:\ProgramData\<tool>\config.toml` on
/// Windows.
pub fn system_config_path(tool: &str) -> PathBuf {
    // TODO: the base path for Windows is hardcoded; it should be determined
    // via the system API to support a general volume label.
    #[cfg(target_os = "windows")]
    let base_path = PathBuf::from("C:\\ProgramData");
    #[cfg(not(target_os = "windows"))]
    let base_path = PathBuf::from("/etc");

    base_path.join(tool).join(CONFIG_FILE_NAME)
}

/// The per-user configuration files for a tool, from lowest to highest
/// precedence. Paths are returned regardless of whether the files exist.
pub fn user_config_paths(tool: &str) -> Vec<PathBuf> {
    [
        // On macOS, honor an explicitly set XDG_CONFIG_HOME even though it
        // is not part of the platform convention used by `dirs`.
        #[cfg(target_os = "macos")]
        std::env::var("XDG_CONFIG_HOME")
            .ok()
            .map(|d| PathBuf::from(d).join(tool).join(CONFIG_FILE_NAME)),
        dirs::config_dir().map(|d| d.join(tool).join(CONFIG_FILE_NAME)),
        tool_home(tool).map(|d| d.join(CONFIG_FILE_NAME)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The home directory of a tool: `$<TOOL>_HOME` if set, otherwise
/// `~/.<tool>`.
pub fn tool_home(tool: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(home_env_var(tool)) {
        Some(PathBuf::from(path))
    } else {
        dirs::home_dir().map(|home| home.join(format!(".{tool}")))
    }
}

/// The system-wide shared configuration file:
/// `/etc/rattler/config.toml`, or `C:\ProgramData\rattler\config.toml` on
/// Windows.
pub fn shared_system_config_path() -> PathBuf {
    system_config_path(SHARED_CONFIG_DIR)
}

/// The per-user shared configuration files, from lowest to highest
/// precedence. Unlike [`user_config_paths`], `$RATTLER_HOME/config.toml` is
/// only included when the environment variable is set: the shared layer has
/// no `~/.rattler` fallback.
pub fn shared_user_config_paths() -> Vec<PathBuf> {
    [
        // On macOS, honor an explicitly set XDG_CONFIG_HOME even though it
        // is not part of the platform convention used by `dirs`.
        #[cfg(target_os = "macos")]
        std::env::var("XDG_CONFIG_HOME").ok().map(|d| {
            PathBuf::from(d)
                .join(SHARED_CONFIG_DIR)
                .join(CONFIG_FILE_NAME)
        }),
        dirs::config_dir().map(|d| d.join(SHARED_CONFIG_DIR).join(CONFIG_FILE_NAME)),
        std::env::var_os(home_env_var(SHARED_CONFIG_DIR))
            .map(|home| PathBuf::from(home).join(CONFIG_FILE_NAME)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// All configuration file locations for a tool, from lowest to highest
/// precedence: the system-wide shared file, the system-wide tool file, the
/// per-user shared files, and the per-user tool files. The user always
/// overrides the system, and within each level the tool file overrides the
/// shared one.
///
/// The returned paths are candidates; they are not checked for existence.
/// Duplicates are removed, keeping the occurrence with the highest
/// precedence; a path that appears in both layers (e.g. `RATTLER_HOME`
/// pointing into a tool's directory) is parsed as a tool file, since the
/// tool layer accepts a superset of the shared keys.
pub fn config_search_paths(tool: &str) -> Vec<ConfigLocation> {
    let mut locations: Vec<ConfigLocation> = [(shared_system_config_path(), ConfigLayer::Shared)]
        .into_iter()
        .chain([(system_config_path(tool), ConfigLayer::Tool)])
        .chain(
            shared_user_config_paths()
                .into_iter()
                .map(|path| (path, ConfigLayer::Shared)),
        )
        .chain(
            user_config_paths(tool)
                .into_iter()
                .map(|path| (path, ConfigLayer::Tool)),
        )
        .map(|(path, layer)| ConfigLocation { path, layer })
        .collect();

    let tool_paths: std::collections::HashSet<PathBuf> = locations
        .iter()
        .filter(|location| location.layer == ConfigLayer::Tool)
        .map(|location| location.path.clone())
        .collect();

    // Deduplicate by path, keeping the *last* occurrence (highest
    // precedence). A path that also appears in the tool layer keeps the
    // `Tool` parse mode regardless of which occurrence survives.
    let mut seen = std::collections::HashSet::new();
    let mut deduped: Vec<ConfigLocation> = locations
        .drain(..)
        .rev()
        .filter(|location| seen.insert(location.path.clone()))
        .map(|location| {
            let layer = if tool_paths.contains(&location.path) {
                ConfigLayer::Tool
            } else {
                ConfigLayer::Shared
            };
            ConfigLocation { layer, ..location }
        })
        .collect();
    deduped.reverse();
    deduped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_env_var_is_derived_from_tool_name() {
        assert_eq!(home_env_var("pixi"), "PIXI_HOME");
        assert_eq!(home_env_var("rattler-build"), "RATTLER_BUILD_HOME");
    }

    #[test]
    fn shared_user_paths_have_no_dotdir_fallback() {
        // Without RATTLER_HOME set, the shared layer must not fall back to
        // `~/.rattler` the way `user_config_paths` falls back to `~/.<tool>`.
        if std::env::var_os("RATTLER_HOME").is_none()
            && let Some(home) = dirs::home_dir()
        {
            let dotdir = home.join(".rattler").join(CONFIG_FILE_NAME);
            assert!(
                !shared_user_config_paths().contains(&dotdir),
                "shared layer must not use a ~/.rattler dotdir"
            );
        }
    }

    #[test]
    fn search_paths_interleave_layers_by_level() {
        let locations = config_search_paths("pixi");
        let position = |path: &std::path::Path| locations.iter().position(|l| l.path == path);

        let system_shared = position(&shared_system_config_path());
        let system_tool = position(&system_config_path("pixi"));
        let user_shared = shared_user_config_paths().first().and_then(|p| position(p));
        let user_tool = user_config_paths("pixi").first().and_then(|p| position(p));

        if let (Some(system_shared), Some(system_tool)) = (system_shared, system_tool) {
            assert!(
                system_shared < system_tool,
                "system tool config must override system shared config"
            );
        }
        if let (Some(system_tool), Some(user_shared)) = (system_tool, user_shared) {
            assert!(
                system_tool < user_shared,
                "user shared config must override system tool config"
            );
        }
        if let (Some(user_shared), Some(user_tool)) = (user_shared, user_tool) {
            assert!(
                user_shared < user_tool,
                "user tool config must override user shared config"
            );
        }
    }

    #[test]
    fn search_paths_mark_layers() {
        let locations = config_search_paths("pixi");
        let tool_paths: Vec<PathBuf> = [system_config_path("pixi")]
            .into_iter()
            .chain(user_config_paths("pixi"))
            .collect();
        for location in &locations {
            let is_shared_path = location.path == shared_system_config_path()
                || shared_user_config_paths().contains(&location.path);
            // A path in both layers is parsed as a tool file.
            match location.layer {
                ConfigLayer::Shared => {
                    assert!(is_shared_path && !tool_paths.contains(&location.path));
                }
                ConfigLayer::Tool => assert!(tool_paths.contains(&location.path)),
            }
        }
    }
}
