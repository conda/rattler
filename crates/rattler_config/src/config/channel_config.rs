use rattler_conda_types::ChannelConfig;

pub fn default_channel_config() -> ChannelConfig {
    ChannelConfig::default_with_root_dir(root_dir())
}

#[cfg(not(target_arch = "wasm32"))]
fn root_dir() -> std::path::PathBuf {
    std::env::current_dir().expect("Could not retrieve the current directory")
}

/// There is no current directory in the browser, so local channel paths are
/// resolved against an empty root instead.
#[cfg(target_arch = "wasm32")]
fn root_dir() -> std::path::PathBuf {
    std::path::PathBuf::new()
}
