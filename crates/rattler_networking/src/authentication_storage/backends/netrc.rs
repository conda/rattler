//! Read authentication credentials from `.netrc` files.

use crate::{
    authentication_storage::{AuthenticationStorageError, StorageBackend},
    Authentication,
};
use netrc_rs::{Machine, Netrc};
use std::{collections::HashMap, env, io::ErrorKind, path::Path, path::PathBuf};

/// Returns the default path for the netrc file — `~/_netrc` on Windows and
/// `~/.netrc` elsewhere. Falls back to `.netrc` in the current directory if
/// the home directory cannot be determined.
fn default_netrc_path() -> PathBuf {
    let Some(mut path) = dirs::home_dir() else {
        return PathBuf::from(".netrc");
    };
    #[cfg(windows)]
    path.push("_netrc");
    #[cfg(not(windows))]
    path.push(".netrc");
    path
}

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
    #[error("could not parse .netrc file: {0}")]
    ParseError(netrc_rs::Error),

    /// Something is not supported
    #[error("{0}")]
    NotSupportedError(String),
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
        // Get the path to the netrc file. If the user explicitly set `NETRC`
        // we remember that so that a missing file surfaces as an error — they
        // asked for a specific file, so silently ignoring it would be
        // surprising.
        let (path, explicit) = if let Ok(val) = env::var("NETRC") {
            tracing::debug!(
                "\"NETRC\" environment variable set, using netrc file at {}",
                val
            );
            (PathBuf::from(val), true)
        } else {
            (default_netrc_path(), false)
        };

        match Self::from_path(&path) {
            Ok(storage) => Ok(storage),
            Err(NetRcStorageError::IOError(err))
                if err.kind() == ErrorKind::NotFound && !explicit =>
            {
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
    fn store(
        &self,
        _host: &str,
        _authentication: &Authentication,
    ) -> Result<(), AuthenticationStorageError> {
        Err(NetRcStorageError::NotSupportedError(
            "NetRcStorage does not support storing credentials".to_string(),
        ))?
    }

    fn delete(&self, _host: &str) -> Result<(), AuthenticationStorageError> {
        Err(NetRcStorageError::NotSupportedError(
            "NetRcStorage does not support deleting credentials".to_string(),
        ))?
    }

    fn get(&self, host: &str) -> Result<Option<Authentication>, AuthenticationStorageError> {
        match self.get_password(host) {
            Ok(Some(auth)) => Ok(Some(auth)),
            Ok(None) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AuthenticationStorage;
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

    /// When `NETRC` points to a malformed file we expect `from_env` to return
    /// `Err` so that the caller (`AuthenticationStorage::from_env_and_defaults`)
    /// can emit a `tracing::warn!`. This is the sanity-check case.
    #[test]
    fn test_from_env_malformed_netrc_returns_err() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".netrc-malformed");
        std::fs::write(&path, b"this is not a valid netrc file !!!!").unwrap();

        temp_env::with_var("NETRC", Some(path.as_os_str()), || {
            let result = NetRcStorage::from_env();
            assert!(
                result.is_err(),
                "expected malformed netrc file to surface an error so the \
                 caller can log a warning",
            );
        });
    }

    /// If `NETRC` is explicitly set, a missing file must surface as `Err` so
    /// the caller can log a warning. The leniency for missing files only
    /// applies to the default `~/.netrc` fallback path.
    #[test]
    fn test_from_env_missing_explicit_netrc_returns_err() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.netrc");
        assert!(!missing.exists());

        temp_env::with_var("NETRC", Some(missing.as_os_str()), || {
            let result = NetRcStorage::from_env();
            assert!(
                result.is_err(),
                "explicit NETRC pointing at a missing file must return Err",
            );
        });
    }

    /// Full-stack check using `tracing_test`: set `NETRC` to a malformed file
    /// and verify that `AuthenticationStorage::from_env_and_defaults` actually
    /// emits the warning. This locks the wiring between the two modules.
    #[test]
    #[tracing_test::traced_test]
    fn test_from_env_and_defaults_warns_on_malformed_netrc() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".netrc-malformed");
        std::fs::write(&path, b"this is not a valid netrc file !!!!").unwrap();

        temp_env::with_vars(
            [
                ("NETRC", Some(path.as_os_str())),
                ("RATTLER_AUTH_FILE", None),
            ],
            || {
                let _ = AuthenticationStorage::from_env_and_defaults();
            },
        );

        assert!(
            logs_contain("error reading netrc file"),
            "expected a tracing::warn! about the malformed netrc file",
        );
    }

    /// Counterpart to the test above: if the warning is missing when `NETRC`
    /// points at a non-existent file, this test will fail — pinning down the
    /// exact scenario the user reported.
    #[test]
    #[tracing_test::traced_test]
    fn test_from_env_and_defaults_warns_on_missing_explicit_netrc() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.netrc");

        temp_env::with_vars(
            [
                ("NETRC", Some(missing.as_os_str())),
                ("RATTLER_AUTH_FILE", None),
            ],
            || {
                let _ = AuthenticationStorage::from_env_and_defaults();
            },
        );

        assert!(
            logs_contain("error reading netrc file"),
            "no tracing::warn! was emitted for an explicitly-set NETRC that \
             points to a missing file — this is the bug: `from_env` swallows \
             ErrorKind::NotFound even when the path comes from the env var",
        );
    }
}
