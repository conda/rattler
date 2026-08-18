/// The location in the main cache folder where the conda package cache is stored.
pub const PACKAGE_CACHE_DIR: &str = "pkgs";
pub const RUN_EXPORTS_CACHE_DIR: &str = "run-exports";
/// The location in the main cache folder where the repodata cache is stored.
pub const REPODATA_CACHE_DIR: &str = "repodata";
pub const EXEC_ENVS_DIR: &str = "exec";
/// The location where virtual package detection plugin results are stored.
#[cfg(feature = "experimental-virtual-package-plugins")]
pub const VIRTUAL_PACKAGE_PLUGINS_CACHE_DIR: &str = "virtual-package-plugins";
