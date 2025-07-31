use rattler_networking::{
    authentication_storage::{self, AuthenticationStorageError},
    AuthenticationStorage,
};
use std::{path::PathBuf, sync::Arc};

/// Get the authentication storage from the given file
pub fn get_auth_store(
    auth_file: Option<PathBuf>,
    auth_store: Option<Result<AuthenticationStorage, AuthenticationStorageError>>,
) -> Result<AuthenticationStorage, AuthenticationStorageError> {
    match auth_store {
        Some(auth_store) => auth_store,
        None => match auth_file {
            Some(auth_file) => {
                let mut store = AuthenticationStorage::empty();
                store.add_backend(Arc::from(
                    authentication_storage::backends::file::FileStorage::from_path(auth_file)?,
                ));
                Ok(store)
            }
            None => rattler_networking::AuthenticationStorage::from_env_and_defaults(),
        },
    }
}

/// The user agent to use for the reqwest client
pub const APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);
