//! Fallback storage for passwords.
use netrc_rs::{Machine, Netrc};
use std::{collections::HashMap, env, io::Read, path::PathBuf};

/// A struct that implements storage and access of authentication
/// information backed by a on-disk JSON file
#[derive(Clone)]
pub struct NetRcStorage {
    /// The netrc file contents
    machines: HashMap<String, Machine>,
}

/// An error that can occur when accessing the fallback storage
#[derive(thiserror::Error, Debug)]
pub enum NetRcStorageError {
    /// An IO error occurred when accessing the fallback storage
    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),
    // /// An error occurred when (de)serializing the credentials
    // #[error("JSON error: {0}")]
    // JSONError(#[from] serde_json::Error),
}

impl NetRcStorage {
    /// Create a new fallback storage by retrieving the netrc file from the user environment.  
    /// This uses the same environment variable as curl and will read the file from $NETRC
    /// falling back to `~/.netrc`
    pub fn from_env() -> Self {
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

        Self {
            // Attempt to read the file and parse the netrc structure.
            // If the file doesn't exist or is invalid, return an empty HashMap
            machines: match std::fs::read_to_string(path) {
                Ok(content) => match Netrc::parse(&content, false) {
                    Ok(netrc) => netrc
                        .machines
                        .into_iter()
                        .map(|m| (m.name.clone(), m))
                        .filter_map(|(name, value)| name.map(|n| (n, value)))
                        .collect(),
                    Err(_) => HashMap::new(),
                },
                Err(_) => HashMap::new(),
            },
        }
    }

    /// Retrieve the authentication information for the given host
    pub fn get_password(&self, host: &str) -> Result<Option<String>, NetRcStorageError> {
        match self.machines.get(host) {
            Some(machine) => Ok(machine.password.clone()),
            None => Ok(None),
        }
    }
}
