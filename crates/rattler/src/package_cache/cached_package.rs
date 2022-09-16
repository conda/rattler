use std::path::{Path, PathBuf};

/// A cached package somewhere on disk.
pub struct CachedPackage {
    path: PathBuf,
}

impl CachedPackage {
    /// Constructs a new instance with the specified path. The path must refer to a valid directory.
    pub(super) fn new(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().canonicalize()?;
        if !path.is_dir() {
            anyhow::bail!("the specified path does not refer to a valid directory");
        }
        Ok(Self { path })
    }

    /// Returns the root directory
    pub fn root(&self) -> &Path {
        self.path.as_path()
    }
}
