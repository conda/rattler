//! Standard configuration file locations shared by rattler-based tools.
//!
//! Every tool has three conventional configuration locations, from lowest to
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
//! [`config_search_paths`] combines these for a *list* of tools so that a
//! tool can layer its own configuration on top of the configuration of the
//! tools it cooperates with — e.g. `rattler-build` passing
//! `&["pixi", "rattler-build"]` reads pixi's global configuration and
//! overrides it with its own.

use std::path::PathBuf;

/// The conventional file name of a configuration file.
pub const CONFIG_FILE_NAME: &str = "config.toml";

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

/// All configuration file locations for the given tools, from lowest to
/// highest precedence: first the system-wide files of every tool, then the
/// per-user files of every tool. Within each group, later tools in the list
/// take precedence over earlier ones.
///
/// The returned paths are candidates; they are not checked for existence.
/// Duplicates (e.g. from overlapping tool homes) are removed, keeping the
/// occurrence with the highest precedence.
pub fn config_search_paths(tools: &[&str]) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = tools
        .iter()
        .map(|tool| system_config_path(tool))
        .chain(tools.iter().flat_map(|tool| user_config_paths(tool)))
        .collect();

    // Deduplicate, keeping the *last* occurrence (highest precedence).
    let mut seen = std::collections::HashSet::new();
    let mut deduped: Vec<PathBuf> = paths
        .drain(..)
        .rev()
        .filter(|path| seen.insert(path.clone()))
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
    fn search_paths_order_system_before_user() {
        let paths = config_search_paths(&["pixi", "rattler-build"]);
        let system_pixi = system_config_path("pixi");
        let user_pixi = user_config_paths("pixi");

        let system_pos = paths.iter().position(|p| p == &system_pixi);
        let user_pos = user_pixi
            .first()
            .and_then(|first| paths.iter().position(|p| p == first));

        if let (Some(system_pos), Some(user_pos)) = (system_pos, user_pos) {
            assert!(
                system_pos < user_pos,
                "system config must have lower precedence than user config"
            );
        }
    }

    #[test]
    fn search_paths_order_within_user_group_follows_tool_order() {
        let paths = config_search_paths(&["pixi", "rattler-build"]);
        let pixi_user = user_config_paths("pixi");
        let rb_user = user_config_paths("rattler-build");

        if let (Some(pixi_first), Some(rb_last)) = (pixi_user.first(), rb_user.last()) {
            let pixi_pos = paths.iter().position(|p| p == pixi_first);
            let rb_pos = paths.iter().position(|p| p == rb_last);
            if let (Some(pixi_pos), Some(rb_pos)) = (pixi_pos, rb_pos) {
                assert!(
                    pixi_pos < rb_pos,
                    "later tools must take precedence over earlier ones"
                );
            }
        }
    }
}
