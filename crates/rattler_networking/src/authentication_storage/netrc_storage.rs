//! Fallback storage for passwords.
use std::{
    env,
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    io::Read,
};
use netrc_rs::{Netrc, Machine};
use serde::de::value;

/// A struct that implements storage and access of authentication
/// information backed by a on-disk JSON file
#[derive(Clone)]
pub struct NetRcStorage {
    /// The path to the JSON file
    pub path: PathBuf,

    /// A mutex to ensure that only one thread accesses the file at a time
    mutex: Arc<Mutex<()>>,

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
    /// Create a new fallback storage with the given path
    pub fn new() -> Self {

        // Get the path to the netrc file
        let path = match env::var("NETRC") {
            Ok(val) => PathBuf::from(val),
            Err(_) => {
                match dirs::home_dir() {
                    Some(mut path) => {
                        path.push(".netrc");
                        path
                    },
                    None => PathBuf::from(".netrc")
                }
            }
        };

        Self {
            path: path.clone(),
            mutex: Arc::new(Mutex::new(())),
            machines: if path.exists() {
                match std::fs::File::open(&path) {
                    Ok(file) => {
                        let mut reader = std::io::BufReader::new(file);
                        let mut content = String::new();
                        match reader.read_to_string(&mut content) {
                            Ok(_) => {
                                match Netrc::parse(&content, false) {
                                    Ok(netrc) => 
                                        netrc.machines.into_iter().map(|m| (m.name.clone(), m)).
                                        filter_map(|(name, value)| name.map(|n| (n, value))).collect(),
                                    Err(_) => HashMap::new()
                                }
                            },
                            Err(_) => HashMap::new()
                        }
                    } 
                    Err(_) => HashMap::new()
                }
            }
            else {
                HashMap::new()
            },
        }
    }

    /// Retrieve the authentication information for the given host
    pub fn get_password(&self, host: &str) -> Result<Option<String>, NetRcStorageError> {
        let _lock = self.mutex.lock().unwrap();        
        match self.machines.get(host) {
            Some(machine) => Ok(machine.password.clone()),
            None => Ok(None),
        }
    }

}
