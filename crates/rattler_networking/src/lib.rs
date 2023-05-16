// #![deny(missing_docs)]

//! Networking utilities for Rattler, specifically authenticating requests

use std::{collections::HashMap, str::FromStr, path::PathBuf};

mod fallback_storage;
use keyring::Entry;
use reqwest::{Client, IntoUrl, Method, Url};
use serde::{Deserialize, Serialize};

/// The different Authentication methods that are supported in the conda
/// ecosystem
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Authentication {
    /// A bearer token is sent as a header of the form
    /// `Authorization: Bearer {TOKEN}`
    BearerToken(String),
    /// A basic authentication token is sent as HTTP basic auth
    Basic {
        /// The username to use for basic auth
        username: String,
        /// The password to use for basic auth
        password: String,
    },
    /// A conda token is sent in the URL as `/t/{TOKEN}/...`
    CondaToken(String),
}

/// An error that can occur when parsing an authentication string
#[derive(Debug)]
pub enum AuthenticationParseError {
    /// The scheme is not valid
    InvalidScheme,
    /// The token could not be parsed
    InvalidToken,
}

/// A struct that implements storage and access of authentication
/// information
#[derive(Clone)]
pub struct AuthenticationStorage {
    /// The store_key needs to be unique per program as it is stored
    /// in a global dictionary in the operating system
    pub store_key: String,

    /// Fallback JSON location
    pub fallback_json_location: PathBuf,

    /// A cache of authentication information
    authentication_cache: HashMap<String, Option<Authentication>>,
}

impl AuthenticationStorage {
    /// Create a new authentication storage with the given store key
    pub fn new(store_key: &str) -> AuthenticationStorage {

        keyring::set_default_credential_builder(Box::new(fallback_storage::JsonFileCredentialBuilder::new(
            "auth_store.json",
        )));

        AuthenticationStorage {
            store_key: store_key.to_string(),
            fallback_json_location: PathBuf::from("auth_store.json"),
            authentication_cache: Default::default(),
        }
    }
}

impl FromStr for Authentication {
    type Err = AuthenticationParseError;

    /// Parse an authentication string into an Authentication struct
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        serde_json::from_str(s).map_err(|_| AuthenticationParseError::InvalidToken)
    }
}

/// An error that can occur when accessing the authentication storage
#[derive(thiserror::Error, Debug)]
pub enum AuthenticationStorageError {
    /// An error occurred when accessing the authentication storage
    #[error("Could not retrieve credentials from authentication storage: {0}")]
    StorageError(#[from] keyring::Error),

    /// An error occurred when serializing the credentials
    #[error("Could not serialize credentials {0}")]
    SerializeCredentialsError(#[from] serde_json::Error),

    /// An error occurred when parsing the credentials
    #[error("Could not parse credentials stored for {host}")]
    ParseCredentialsError {
        /// The host for which the credentials could not be parsed
        host: String,
    },
}

impl AuthenticationStorage {
    /// Store the given authentication information for the given host
    pub fn store(
        &self,
        host: &str,
        authentication: &Authentication,
    ) -> Result<(), AuthenticationStorageError> {
        let entry = Entry::new(&self.store_key, host)?;
        entry.set_password(&serde_json::to_string(authentication)?)?;
        Ok(())
    }

    /// Retrieve the authentication information for the given host
    pub fn get(&self, host: &str) -> Result<Option<Authentication>, AuthenticationStorageError> {
        if let Some(cached) = self.authentication_cache.get(host) {
            return Ok(cached.clone());
        }

        let entry = Entry::new(&self.store_key, host)?;
        let password = entry.get_password();

        match password {
            Ok(password) => Ok(Some(Authentication::from_str(&password).map_err(|_| {
                AuthenticationStorageError::ParseCredentialsError {
                    host: host.to_string(),
                }
            })?)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(AuthenticationStorageError::StorageError(e)),
        }
    }

    /// Retrieve the authentication information for the given URL
    pub fn get_by_url<U: IntoUrl>(
        &self,
        url: U,
    ) -> Result<(Url, Option<Authentication>), reqwest::Error> {
        let url = url.into_url()?;

        if let Some(host) = url.host_str() {
            let credentials = self.get(host);
            match credentials {
                Ok(None) => Ok((url, None)),
                Ok(Some(credentials)) => Ok((url, Some(credentials))),
                Err(e) => {
                    tracing::warn!("Error retrieving credentials for {}: {}", host, e);
                    Ok((url, None))
                }
            }
        } else {
            Ok((url, None))
        }
    }

    /// Delete the authentication information for the given host
    pub fn delete(&self, host: &str) -> keyring::Result<()> {
        let entry = Entry::new(&self.store_key, host)?;
        entry.delete_password()
    }
}

/// A client that can be used to make authenticated requests, based on the [`reqwest::Client`]
#[derive(Clone)]
pub struct AuthenticatedClient {
    /// The underlying client
    client: Client,

    /// The authentication storage
    auth_storage: AuthenticationStorage,
}

impl Default for AuthenticatedClient {
    fn default() -> Self {
        AuthenticatedClient {
            client: Client::default(),
            auth_storage: AuthenticationStorage::new("rattler"),
        }
    }
}

impl AuthenticatedClient {
    /// Create a new authenticated client from the given client and authentication storage
    pub fn from_client(client: Client, auth_storage: AuthenticationStorage) -> AuthenticatedClient {
        AuthenticatedClient {
            client,
            auth_storage,
        }
    }
}

impl AuthenticatedClient {
    /// Create a GET request builder for the given URL (see also [`reqwest::Client::get`])
    pub fn get<U: IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        self.request(Method::GET, url)
    }

    /// Create a POST request builder for the given URL (see also [`reqwest::Client::post`])
    pub fn post<U: IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        self.request(Method::POST, url)
    }

    /// Create a HEAD request builder for the given URL (see also [`reqwest::Client::head`])
    pub fn head<U: IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        self.request(Method::HEAD, url)
    }

    /// Create a request builder for the given URL (see also [`reqwest::Client::request`])
    pub fn request<U: IntoUrl>(&self, method: Method, url: U) -> reqwest::RequestBuilder {
        let url_clone = url.as_str().to_string();
        match self.auth_storage.get_by_url(url) {
            Err(_) => {
                // forward error to caller (invalid URL)
                self.client.request(method, url_clone)
            }
            Ok((url, auth)) => {
                let url = self.authenticate_url(url, &auth);
                let request_builder = self.client.request(method, url);
                self.authenticate_request(request_builder, &auth)
            }
        }
    }

    /// Authenticate the given URL with the given authentication information
    fn authenticate_url(&self, url: Url, auth: &Option<Authentication>) -> Url {
        if let Some(credentials) = auth {
            match credentials {
                Authentication::CondaToken(token) => {
                    let path = url.path();
                    let mut new_path = String::new();
                    new_path.push_str(format!("/t/{}", token).as_str());
                    new_path.push_str(path);
                    let mut url = url.clone();
                    url.set_path(&new_path);
                    url
                }
                _ => url,
            }
        } else {
            url
        }
    }

    /// Authenticate the given request builder with the given authentication information
    fn authenticate_request(
        &self,
        builder: reqwest::RequestBuilder,
        auth: &Option<Authentication>,
    ) -> reqwest::RequestBuilder {
        if let Some(credentials) = auth {
            match credentials {
                Authentication::BearerToken(token) => builder.bearer_auth(token),
                Authentication::Basic { username, password } => {
                    builder.basic_auth(username, Some(password))
                }
                Authentication::CondaToken(_) => builder,
            }
        } else {
            builder
        }
    }
}

/// A blocking client that can be used to make authenticated requests, based on the [`reqwest::blocking::Client`]
pub struct AuthenticatedClientBlocking {
    /// The underlying client
    client: reqwest::blocking::Client,

    /// The authentication storage
    auth_storage: AuthenticationStorage,
}

impl AuthenticatedClientBlocking {
    /// Create a new authenticated client from the given client and authentication storage
    pub fn from_client(
        client: reqwest::blocking::Client,
        auth_storage: AuthenticationStorage,
    ) -> AuthenticatedClientBlocking {
        AuthenticatedClientBlocking {
            client,
            auth_storage,
        }
    }
}

impl Default for AuthenticatedClientBlocking {
    fn default() -> Self {
        AuthenticatedClientBlocking {
            client: Default::default(),
            auth_storage: AuthenticationStorage::new("rattler"),
        }
    }
}

impl AuthenticatedClientBlocking {
    /// Create a GET request builder for the given URL (see also [`reqwest::blocking::Client::get`])
    pub fn get<U: IntoUrl>(&self, url: U) -> reqwest::blocking::RequestBuilder {
        self.request(Method::GET, url)
    }

    /// Create a POST request builder for the given URL (see also [`reqwest::blocking::Client::post`])
    pub fn post<U: IntoUrl>(&self, url: U) -> reqwest::blocking::RequestBuilder {
        self.request(Method::POST, url)
    }

    /// Create a HEAD request builder for the given URL (see also [`reqwest::blocking::Client::head`])
    pub fn head<U: IntoUrl>(&self, url: U) -> reqwest::blocking::RequestBuilder {
        self.request(Method::HEAD, url)
    }

    /// Create a request builder for the given URL (see also [`reqwest::blocking::Client::request`])
    pub fn request<U: IntoUrl>(&self, method: Method, url: U) -> reqwest::blocking::RequestBuilder {
        let url_clone = url.as_str().to_string();
        match self.auth_storage.get_by_url(url) {
            Err(_) => {
                // forward error to caller (invalid URL)
                self.client.request(method, url_clone)
            }
            Ok((url, auth)) => {
                let url = self.authenticate_url(url, &auth);
                let request_builder = self.client.request(method, url);
                self.authenticate_request(request_builder, &auth)
            }
        }
    }

    /// Authenticate the given URL with the given authentication information
    fn authenticate_url(&self, url: Url, auth: &Option<Authentication>) -> Url {
        if let Some(credentials) = auth {
            match credentials {
                Authentication::CondaToken(token) => {
                    let path = url.path();
                    let mut new_path = String::new();
                    new_path.push_str(format!("/t/{}", token).as_str());
                    new_path.push_str(path);
                    let mut url = url.clone();
                    url.set_path(&new_path);
                    url
                }
                _ => url,
            }
        } else {
            url
        }
    }

    /// Authenticate the given request builder with the given authentication information
    fn authenticate_request(
        &self,
        builder: reqwest::blocking::RequestBuilder,
        auth: &Option<Authentication>,
    ) -> reqwest::blocking::RequestBuilder {
        if let Some(credentials) = auth {
            match credentials {
                Authentication::BearerToken(token) => builder.bearer_auth(token),
                Authentication::Basic { username, password } => {
                    builder.basic_auth(username, Some(password))
                }
                Authentication::CondaToken(_) => builder,
            }
        } else {
            builder
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_fallback() -> anyhow::Result<()> {
        println!("Starting up");
        let storage = super::AuthenticationStorage::new("rattler_test");
        println!("Storage created");
        let host = "test.example.com";
        let authentication = Authentication::CondaToken("testtoken".to_string());
        println!("Storage storing");
        storage.store(host, &authentication)?;
        println!("Storage done");
        Ok(())
    }

    #[test]
    fn test_conda_token_storage() -> anyhow::Result<()> {
        let storage = super::AuthenticationStorage::new("rattler_test");
        let host = "test.example.com";
        let retrieved = storage.get(host);

        if let Err(e) = retrieved.as_ref() {
            println!("{:?}", e);
        }

        assert!(retrieved.is_ok());
        assert!(retrieved.unwrap().is_none());

        let authentication = Authentication::CondaToken("testtoken".to_string());
        storage.store(host, &authentication)?;

        let retrieved = storage.get(host);
        assert!(retrieved.is_ok());
        let retrieved = retrieved.unwrap();
        assert!(retrieved.is_some());
        let auth = retrieved.unwrap();
        assert!(auth == authentication);

        let client = AuthenticatedClient::from_client(reqwest::Client::default(), storage.clone());
        let request = client.get("https://test.example.com/conda-forge/noarch/testpkg.tar.bz2");
        let request = request.build().unwrap();
        let url = request.url();
        assert!(url.path().starts_with("/t/testtoken"));

        storage.delete(host)?;
        Ok(())
    }

    #[test]
    fn test_bearer_storage() -> anyhow::Result<()> {
        let storage = super::AuthenticationStorage::new("rattler_test");
        let host = "bearer.example.com";
        let retrieved = storage.get(host);

        if let Err(e) = retrieved.as_ref() {
            println!("{:?}", e);
        }

        assert!(retrieved.is_ok());
        assert!(retrieved.unwrap().is_none());

        let authentication = Authentication::BearerToken("xyztokytoken".to_string());
        storage.store(host, &authentication)?;

        let retrieved = storage.get(host);
        assert!(retrieved.is_ok());
        let retrieved = retrieved.unwrap();
        assert!(retrieved.is_some());
        let auth = retrieved.unwrap();
        assert!(auth == authentication);

        let client = AuthenticatedClient::from_client(reqwest::Client::default(), storage.clone());
        let request = client.get("https://bearer.example.com/conda-forge/noarch/testpkg.tar.bz2");
        let request = request.build().unwrap();
        let url = request.url();
        assert!(url.to_string() == "https://bearer.example.com/conda-forge/noarch/testpkg.tar.bz2");
        assert_eq!(
            request.headers().get("Authorization").unwrap(),
            "Bearer xyztokytoken"
        );

        storage.delete(host)?;
        Ok(())
    }

    #[test]
    fn test_basic_auth_storage() -> anyhow::Result<()> {
        let storage = super::AuthenticationStorage::new("rattler_test");
        let host = "basic.example.com";
        let retrieved = storage.get(host);

        if let Err(e) = retrieved.as_ref() {
            println!("{:?}", e);
        }

        assert!(retrieved.is_ok());
        assert!(retrieved.unwrap().is_none());

        let authentication = Authentication::Basic {
            username: "testuser".to_string(),
            password: "testpassword".to_string(),
        };
        storage.store(host, &authentication)?;

        let retrieved = storage.get(host);
        assert!(retrieved.is_ok());
        let retrieved = retrieved.unwrap();
        assert!(retrieved.is_some());
        let auth = retrieved.unwrap();
        assert!(auth == authentication);

        let client = AuthenticatedClient::from_client(reqwest::Client::default(), storage.clone());
        let request = client.get("https://basic.example.com/conda-forge/noarch/testpkg.tar.bz2");
        let request = request.build().unwrap();
        let url = request.url();
        assert!(url.to_string() == "https://basic.example.com/conda-forge/noarch/testpkg.tar.bz2");
        assert_eq!(
            request.headers().get("Authorization").unwrap(),
            // this is the base64 encoding of "testuser:testpassword"
            "Basic dGVzdHVzZXI6dGVzdHBhc3N3b3Jk"
        );

        storage.delete(host)?;
        Ok(())
    }
}
