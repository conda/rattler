//! Read authentication credentials from `.netrc` files.

use crate::{authentication_storage::StorageBackend, Authentication};
use netrc_rs::{Machine, Netrc};
use std::{collections::HashMap, env, io::ErrorKind, path::Path, path::PathBuf};

/// A struct that implements storage and access of authentication
/// information backed by a on-disk JSON file
#[derive(Debug, Clone, Default)]
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
                    #[cfg(windows)]
                    path.push("_netrc");
                    #[cfg(not(windows))]
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
    pub fn get_password(&self, host: &str) -> Result<Option<Authentication>, NetRcStorageError> {
        match self.machines.get(host) {
            Some(machine) => Ok(Some(Authentication::BasicHTTP {
                username: machine.login.clone().unwrap_or_default(),
                password: machine.password.clone().unwrap_or_default(),
            })),
            None => Ok(None),
        }
    }
}

impl StorageBackend for NetRcStorage {
    fn store(&self, _host: &str, _authentication: &Authentication) -> anyhow::Result<()> {
        anyhow::bail!("NetRcStorage does not support storing credentials")
    }

    fn delete(&self, _host: &str) -> anyhow::Result<()> {
        anyhow::bail!("NetRcStorage does not support deleting credentials")
    }

    fn get(&self, host: &str) -> anyhow::Result<Option<Authentication>> {
        match self.get_password(host) {
            Ok(Some(auth)) => Ok(Some(auth)),
            Ok(None) => Ok(None),
            Err(err) => Err(anyhow::Error::new(err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_file_storage() {
        let file = tempdir().unwrap();
        let path = file.path().join(".testnetrc");

        let mut netrc = std::fs::File::create(&path).unwrap();
        netrc
            .write_all(b"machine mainmachine\nlogin test\npassword password\n")
            .unwrap();
        netrc.flush().unwrap();

        let storage = NetRcStorage::from_path(path.as_path()).unwrap();
        assert_eq!(
            storage.get("mainmachine").unwrap(),
            Some(Authentication::BasicHTTP {
                username: "test".to_string(),
                password: "password".to_string(),
            })
        );

        assert_eq!(storage.get("test_unknown").unwrap(), None);
    }

    #[test]
    fn test_file_storage_from_env() {
        let file = tempdir().unwrap();
        let path = file.path().join(".testnetrc2");

        let mut netrc = std::fs::File::create(&path).unwrap();
        netrc
            .write_all(b"machine supermachine\nlogin test2\npassword password2\n")
            .unwrap();
        netrc.flush().unwrap();

        let old_netrc = env::var("NETRC");
        env::set_var("NETRC", path.as_os_str());

        let storage = NetRcStorage::from_env().unwrap();

        assert_eq!(
            storage.get("supermachine").unwrap(),
            Some(Authentication::BasicHTTP {
                username: "test2".to_string(),
                password: "password2".to_string(),
            })
        );

        assert_eq!(storage.get("test_unknown").unwrap(), None);

        if let Ok(netrc) = old_netrc {
            env::set_var("NETRC", netrc);
        } else {
            env::remove_var("NETRC");
        }
    }
}
