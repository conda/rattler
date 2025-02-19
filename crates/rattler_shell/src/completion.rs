//! Module for handling shell completion scripts.

use std::fmt;
use std::path::{Path, PathBuf};

use rattler_conda_types::Platform;

use crate::shell::{Shell, ShellScript};

/// A struct that holds values for the activation and deactivation
/// process of an environment, e.g. activation scripts to execute or environment
/// variables to set.
#[derive(Debug)]
pub struct ShellCompletionActivator<T: Shell + 'static> {
    /// The target prefix for which the completion scripts are collected.
    pub target_prefix: PathBuf,
    /// The shell type for which the completion scripts are collected.
    pub shell_type: T,
    /// The completion scripts that were found.
    pub completion_scripts: Vec<PathBuf>,
    /// The platform for which the completion scripts are collected.
    pub platform: Platform,
}

/// Collect completion scripts from a given path and shell type.
pub fn collect_completion_scripts(prefix: &Path, shell_type: impl Shell) -> Vec<PathBuf> {
    if let Some(location) = shell_type.completion_script_location() {
        let folder = prefix.join(location);
        let mut scripts = vec![];
        if folder.exists() {
            for entry in folder.read_dir().expect("Failed to read directory") {
                if let Ok(entry) = entry {
                    scripts.push(entry.path());
                }
            }
        }
        scripts
    } else {
        vec![]
    }
}

impl<T: Shell + Clone> ShellCompletionActivator<T> {
    /// Create a new `ShellCompletionActivator` from a given path, shell type and platform.
    pub fn from_path(target_prefix: PathBuf, shell_type: T, platform: Platform) -> Self {
        let completion_scripts = collect_completion_scripts(&target_prefix, shell_type.clone());
        Self {
            target_prefix,
            shell_type,
            completion_scripts,
            platform,
        }
    }

    /// Return a `ShellScript` that runs all the completion scripts.
    pub fn to_script(&self) -> Result<ShellScript<T>, fmt::Error> {
        let mut script = ShellScript::new(self.shell_type.clone(), self.platform);
        for completion_script in &self.completion_scripts {
            script.run_script(&completion_script)?;
        }
        Ok(script)
    }
}
