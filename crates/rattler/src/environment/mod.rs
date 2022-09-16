use anyhow::Context;
use std::path::{Path, PathBuf};
use url::Url;

/// Represents a Conda environment somewhere on disk.
pub struct Environment {
    /// The root directory of the environment
    prefix: PathBuf,
}

impl Environment {
    /// Updates or constructs a new environment containing the specified packages.
    pub async fn create(
        prefix: impl AsRef<Path>,
        packages: impl IntoIterator<Item = Url>,
    ) -> anyhow::Result<Environment> {
        let prefix = prefix.as_ref();

        // First delete the prefix directory and then recreate it.
        // TODO: Simply uninstall packages if possible instead of always recreating
        if prefix.is_dir() {
            std::fs::remove_dir_all(prefix).context("deleting existing environment")?;
        }
        std::fs::create_dir_all(prefix).context("creating environment directory")?;

        // Construct the new environment
        let mut environment = Self::new(prefix)?;

        // Update the environment with the specs
        environment.update(packages).await?;

        Ok(environment)
    }

    /// Opens the environment at the given prefix but does nothing with it. This method does not
    /// check if there is a valid environment at the given prefix. That is only checked when the
    /// environment is queried. However, the specified prefix must refer to an existing directory.
    pub fn new(prefix: impl AsRef<Path>) -> anyhow::Result<Environment> {
        let prefix = prefix.as_ref().canonicalize()?;
        if !prefix.is_dir() {
            anyhow::bail!("prefix must refer to a valid directory");
        }

        Ok(Environment { prefix })
    }

    /// Update the environment by installing the specified packages.
    /// TODO: This method should also support removing packages.
    pub async fn update(&mut self, install: impl IntoIterator<Item = Url>) -> anyhow::Result<()> {
        todo!();
    }
}
