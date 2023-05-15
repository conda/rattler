use std::{collections::HashMap, str::FromStr};

use keyring::Entry;
use reqwest::{Client, IntoUrl, Method, Url};

#[derive(Clone)]
pub enum Authentication {
    BearerToken(String),
    Basic { username: String, password: String },
    CondaToken(String),
}

#[derive(Debug)]
pub enum AuthenticationParseError {
    InvalidScheme,
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
    // pub fallback_json_location: PathBuf,
    authentication_cache: HashMap<String, Option<Authentication>>,
}

impl AuthenticationStorage {
    pub fn new(store_key: &str) -> AuthenticationStorage {
        AuthenticationStorage {
            store_key: store_key.to_string(),
            authentication_cache: Default::default(),
        }
    }
}

impl FromStr for Authentication {
    type Err = AuthenticationParseError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let mut parts = s.split_whitespace();
        let scheme = parts.next().unwrap_or_default();
        let token = parts.next().unwrap_or_default();
        match scheme {
            "Bearer" => Ok(Authentication::BearerToken(token.to_string())),
            "Basic" => {
                let mut token_parts = token.split(':');
                let username = token_parts.next().unwrap_or_default();
                let password = token_parts.next().unwrap_or_default();
                Ok(Authentication::Basic {
                    username: username.to_string(),
                    password: password.to_string(),
                })
            }
            "CondaToken" => Ok(Authentication::CondaToken(token.to_string())),
            _ => Err(AuthenticationParseError::InvalidScheme),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum AuthenticationStorageError {
    #[error("Could not retrieve credentials from authentication storage: {0}")]
    StorageError(#[from] keyring::Error),

    #[error("Could not parse credentials stored for {host}")]
    ParseCredentialsError { host: String },
}

impl AuthenticationStorage {
    pub fn store(&self, host: &str, authentication: &Authentication) -> keyring::Result<()> {
        let entry = Entry::new(&self.store_key, host)?;
        match authentication {
            Authentication::BearerToken(token) => {
                let password = format!("Bearer {}", token);
                entry.set_password(&password)
            }
            Authentication::Basic { username, password } => {
                let password = format!("Basic {}:{}", username, password);
                entry.set_password(&password)
            }
            Authentication::CondaToken(token) => {
                let password = format!("CondaToken {}", token);
                entry.set_password(&password)
            }
        }
    }

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

    pub fn delete(&self, host: &str) -> keyring::Result<()> {
        let entry = Entry::new(&self.store_key, host)?;
        entry.delete_password()
    }
}

#[derive(Clone)]
pub struct AuthenticatedClient {
    client: Client,

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
    pub fn from_client(client: Client, auth_storage: AuthenticationStorage) -> AuthenticatedClient {
        AuthenticatedClient {
            client,
            auth_storage,
        }
    }
}

impl AuthenticatedClient {
    pub fn get<U: IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        self.request(Method::GET, url)
    }

    pub fn post<U: IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        self.request(Method::POST, url)
    }

    pub fn head<U: IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        self.request(Method::HEAD, url)
    }

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

pub struct AuthenticatedClientBlocking {
    client: reqwest::blocking::Client,

    auth_storage: AuthenticationStorage,
}

impl AuthenticatedClientBlocking {
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
    pub fn get<U: IntoUrl>(&self, url: U) -> reqwest::blocking::RequestBuilder {
        self.request(Method::GET, url)
    }

    pub fn post<U: IntoUrl>(&self, url: U) -> reqwest::blocking::RequestBuilder {
        self.request(Method::POST, url)
    }

    pub fn head<U: IntoUrl>(&self, url: U) -> reqwest::blocking::RequestBuilder {
        self.request(Method::HEAD, url)
    }

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
