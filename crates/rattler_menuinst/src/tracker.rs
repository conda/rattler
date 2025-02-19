use fs_err::File;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, BufReader, BufWriter},
    path::{Path, PathBuf},
};
use thiserror::Error;

/// Track the installation of menu items on the system and make it easy to remove them
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MenuinstTracker {
    Linux(LinuxTracker),
    Windows(WindowsTracker),
    MacOs(MacOsTracker),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LinuxTracker {
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowsTracker {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MacOsTracker {
    pub paths: Vec<PathBuf>,
    pub lsregister: Option<PathBuf>,
}

impl MacOsTracker {
    pub fn new() -> Self {
        Self {
            paths: vec![],
            lsregister: None,
        }
    }
}

/// Errors that can occur when saving or loading the menu installation tracker
#[derive(Debug, Error)]
pub enum TrackerError {
    #[error("Failed to create file: {0}")]
    FileCreate(#[source] io::Error),
    #[error("Failed to read file: {0}")]
    FileRead(#[source] io::Error),
    #[error("Failed to serialize tracker: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("Failed to deserialize tracker: {0}")]
    Deserialize(#[source] serde_json::Error),
}

impl MenuinstTracker {
    /// Saves the menu installation tracker to a JSON file at the specified path.
    ///
    /// # Arguments
    /// * `path` - The path where to save the tracker file
    ///
    /// # Errors
    /// Returns a [`TrackerError`] if:
    /// * The file cannot be created
    /// * The tracker cannot be serialized to JSON
    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<(), TrackerError> {
        let file = File::create(path.as_ref()).map_err(TrackerError::FileCreate)?;
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, self).map_err(TrackerError::Serialize)
    }

    /// Loads a menu installation tracker from a JSON file.
    ///
    /// # Arguments
    /// * `path` - The path to the tracker file to load
    ///
    /// # Errors
    /// Returns a [`TrackerError`] if:
    /// * The file cannot be opened
    /// * The file contains invalid JSON
    /// * The JSON does not represent a valid tracker
    pub fn load_from(path: impl AsRef<Path>) -> Result<Self, TrackerError> {
        let file = File::open(path.as_ref()).map_err(TrackerError::FileRead)?;
        let reader = BufReader::new(file);
        serde_json::from_reader(reader).map_err(TrackerError::Deserialize)
    }
}
