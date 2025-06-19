use std::path::PathBuf;

mod consts {
    pub const CONFIG_DIR: &str = "pixi";
    pub const PIXI_DIR: &str = ".pixi";
    pub const CONFIG_FILE: &str = "config.toml";
}

/// Get pixi home directory, default to `$HOME/.pixi`
///
/// It may be overridden by the `PIXI_HOME` environment variable.
///
/// # Returns
///
/// The pixi home directory
pub fn pixi_home() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PIXI_HOME") {
        Some(PathBuf::from(path))
    } else {
        dirs::home_dir().map(|path| path.join(consts::PIXI_DIR))
    }
}

/// Returns the path(s) to the global pixi config file.
pub fn config_path_global() -> Vec<PathBuf> {
    vec![
        dirs::config_dir().map(|d| d.join(consts::CONFIG_DIR).join(consts::CONFIG_FILE)),
        pixi_home().map(|d| d.join(consts::CONFIG_FILE)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Returns the path to the system-level pixi config file.
pub fn config_path_system() -> PathBuf {
    // TODO: the base_path for Windows is currently hardcoded, it should be
    // determined via the system API to support general volume label
    #[cfg(target_os = "windows")]
    let base_path = PathBuf::from("C:\\ProgramData");
    #[cfg(not(target_os = "windows"))]
    let base_path = PathBuf::from("/etc");

    base_path.join(consts::CONFIG_DIR).join(consts::CONFIG_FILE)
}
