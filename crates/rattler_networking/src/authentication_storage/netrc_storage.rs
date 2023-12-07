//! Fallback storage for passwords.

use netrc_rs::{Machine, Netrc};
use std::{collections::HashMap, env, io::ErrorKind, path::Path, path::PathBuf};

/// A struct that implements storage and access of authentication
/// information backed by a on-disk JSON file
#[derive(Clone, Default)]
pub struct NetRcStorage {
    /// The netrc file contents
    machines: HashMap<String, Machine>,
}

/// An error that can occur when accessing the fallback storage
#[derive(thiserror::Error, Debug)]
pub enum NetRcStorageError {
    /// An IO error occurred when accessing the fallback storage
    #[error(transparent)]
    IOError(#[from] std::io::Error),

    /// An error occurred when parsing the netrc file
    #[error("could not parse .netc file: {0}")]
    ParseError(netrc_rs::Error),
}

impl NetRcStorage {
    /// Create a new fallback storage by retrieving the netrc file from the user environment.  
    /// This uses the same environment variable as curl and will read the file from $NETRC
    /// falling back to `~/.netrc`.
    ///
    /// If reading the file fails or parsing the file fails, this will return an error. However,
    /// if the file does not exist an empty storage will be returned.
    ///
    /// When an error is returned the path to the file that the was read from is returned as well.
    pub fn from_env() -> Result<Self, (PathBuf, NetRcStorageError)> {
        // Get the path to the netrc file
        let path = match env::var("NETRC") {
            Ok(val) => PathBuf::from(val),
            Err(_) => match dirs::home_dir() {
                Some(mut path) => {
                    path.push(".netrc");
                    path
                }
                None => PathBuf::from(".netrc"),
            },
        };

        match Self::from_path(&path) {
            Ok(storage) => Ok(storage),
            Err(NetRcStorageError::IOError(err)) if err.kind() == ErrorKind::NotFound => {
                Ok(Self::default())
            }
            Err(err) => Err((path, err)),
        }
    }

    /// Constructs a new [`NetRcStorage`] by reading the `.netrc` file at the given path. Returns
    /// an error if reading from the file failed or if parsing the file failed.
    pub fn from_path(path: &Path) -> Result<Self, NetRcStorageError> {
        let content = std::fs::read_to_string(path)?;
        let netrc = Netrc::parse(content, false).map_err(NetRcStorageError::ParseError)?;
        let machines = netrc
            .machines
            .into_iter()
            .map(|m| (m.name.clone(), m))
            .filter_map(|(name, value)| name.map(|n| (n, value)))
            .collect();
        Ok(Self { machines })
    }

    /// Retrieve the authentication information for the given host
    pub fn get_password(&self, host: &str) -> Result<Option<String>, NetRcStorageError> {
        match self.machines.get(host) {
            Some(machine) => Ok(machine.password.clone()),
            None => Ok(None),
        }
    }
}
