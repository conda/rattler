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