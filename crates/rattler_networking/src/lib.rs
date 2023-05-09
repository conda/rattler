use std::{collections::HashMap, str::FromStr};

use keyring::Entry;
use reqwest::{Client, IntoUrl, blocking::RequestBuilder, Url};

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
        AuthenticationStorage { store_key: store_key.to_string(), authentication_cache: Default::default() }
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
                let mut token_parts = token.split(":");
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

        let result = match password {
            Ok(password) => Ok(Some(Authentication::from_str(&password).map_err(|_| {
                AuthenticationStorageError::ParseCredentialsError {
                    host: host.to_string(),
                }
            })?)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => return Err(AuthenticationStorageError::StorageError(e)),
        };

        result
    }

    pub fn delete(&self, host: &str) -> keyring::Result<()> {
        let entry = Entry::new(&self.store_key, host)?;
        entry.delete_password()
    }

    // pub fn authenticate_request<U: IntoUrl>(&self, client: &Client, url: U) -> Result<reqwest::RequestBuilder> {
    //     let url = url.into_url()?;

    //     let host = match url.host_str() {
    //         None => return client.get(url),
    //         Some(host) => host,
    //     };

    //     let authentication = get_authentication(host).unwrap();

    //     println!("Getting authenticated request for host: {}", host);

    //     if authentication.is_none() {
    //         println!("No authentication found for host: {}", host);
    //         return client.get(url.clone());
    //     }

    //     let authentication = authentication.unwrap();

    //     match authentication {
    //         Authentication::BearerToken(token) => {
    //             println!("Using bearer token for host: {}", host);
    //             client.get(url).bearer_auth(token)
    //         },
    //         Authentication::Basic { username, password } => {
    //             client.get(url).basic_auth(username, Some(password))
    //         },
    //         Authentication::CondaToken(token) => {
    //             let path = url.path();
    //             let mut new_path = String::new();
    //             new_path.push_str(format!("/t/{}", token).as_str());
    //             new_path.push_str(path);
    //             let mut url = url.clone();
    //             url.set_path(&new_path);
    //             client.get(url)
    //         },
    //     }
    // }
}

// pub fn authenticated_request<U: IntoUrl>(client: &Client, url: U) -> reqwest::RequestBuilder {
//     let url = url.into_url().unwrap();
//     let host = url.host_str().unwrap();

//     let authentication = get_authentication(host).unwrap();

//     println!("Getting authenticated request for host: {}", host);

//     if authentication.is_none() {
//         println!("No authentication found for host: {}", host);
//         return client.get(url.clone());
//     }

//     let authentication = authentication.unwrap();

//     match authentication {
//         Authentication::BearerToken(token) => {
//             println!("Using bearer token for host: {}", host);
//             client.get(url).bearer_auth(token)
//         },
//         Authentication::Basic { username, password } => {
//             client.get(url).basic_auth(username, Some(password))
//         },
//         Authentication::CondaToken(token) => {
//             let path = url.path();
//             let mut new_path = String::new();
//             new_path.push_str(format!("/t/{}", token).as_str());
//             new_path.push_str(path);
//             let mut url = url.clone();
//             url.set_path(&new_path);
//             client.get(url)
//         },
//     }
// }

// pub fn authenticated_request_blocking<U: IntoUrl>(client: &reqwest::blocking::Client, url: U) -> reqwest::blocking::RequestBuilder {
//     let url = url.into_url().unwrap();
//     let host = url.host_str().unwrap();

//     let authentication = get_authentication(host).unwrap();

//     if authentication.is_none() {
//         return client.get(url.clone());
//     }

//     let authentication = authentication.unwrap();

//     match authentication {
//         Authentication::BearerToken(token) => {
//             client.get(url).bearer_auth(token)
//         },
//         Authentication::Basic { username, password } => {
//             client.get(url).basic_auth(username, Some(password))
//         },
//         Authentication::CondaToken(token) => {
//             let path = url.path();
//             let mut new_path = String::new();
//             new_path.push_str(format!("/t/{}", token).as_str());
//             new_path.push_str(path);
//             let mut url = url.clone();
//             url.set_path(&new_path);
//             client.get(url)
//         },
//     }
// }

#[derive(Clone)]
pub struct AuthenticatedClient {
    client: Client,

    auth_storage: AuthenticationStorage,
}

impl AuthenticatedClient {
    pub fn from_client(client: Client, auth_storage: AuthenticationStorage) -> AuthenticatedClient {
        AuthenticatedClient {
            client,
            auth_storage
        }
    }
}

impl AuthenticatedClient {
    pub fn get<U: IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        let url = url.into_url().unwrap();
        self.authenticate(self.client.get(url.clone()), url)
    }

    pub fn post<U: IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        let url = url.into_url().unwrap();
        self.authenticate(self.client.post(url.clone()), url)
    }

    pub fn head<U: IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        let url = url.into_url().unwrap();
        self.authenticate(self.client.head(url.clone()), url)
    }

    fn authenticate(&self, builder: reqwest::RequestBuilder, url: Url) -> reqwest::RequestBuilder {
        if let Some(host) = url.host_str() {
            let credentials = self.auth_storage.get(host);
            let credentials = match credentials {
                Ok(None) => return builder,
                Ok(Some(credentials)) => credentials,
                Err(e) => {
                    tracing::warn!("Error retrieving credentials for {}", host);
                    return builder;    
                },
            };

            match credentials {
                Authentication::BearerToken(token) => {
                    builder.bearer_auth(token)
                },
                Authentication::Basic { username, password } => {
                    builder.basic_auth(username, Some(password))
                },
                Authentication::CondaToken(_token) => {
                    builder
                    // let path = url.path();
                    // let mut new_path = String::new();
                    // new_path.push_str(format!("/t/{}", token).as_str());
                    // new_path.push_str(path);
                    // let mut url = url.clone();
                    // url.set_path(&new_path);
                    // builder.
                },
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
    fn from_client(client: reqwest::blocking::Client, auth_storage: AuthenticationStorage) -> AuthenticatedClientBlocking {
        AuthenticatedClientBlocking {
            client,
            auth_storage
        }
    }
}

impl AuthenticatedClientBlocking {
    pub fn get<U: IntoUrl>(&self, url: U) -> reqwest::blocking::RequestBuilder {
        let url = url.into_url().unwrap();
        self.authenticate(self.client.get(url.clone()), url)
    }

    pub fn post<U: IntoUrl>(&self, url: U) -> reqwest::blocking::RequestBuilder {
        let url = url.into_url().unwrap();
        self.authenticate(self.client.post(url.clone()), url)
    }

    pub fn head<U: IntoUrl>(&self, url: U) -> reqwest::blocking::RequestBuilder {
        let url = url.into_url().unwrap();
        self.authenticate(self.client.head(url.clone()), url)
    }

    fn authenticate(&self, builder: reqwest::blocking::RequestBuilder, url: Url) -> reqwest::blocking::RequestBuilder {
        if let Some(host) = url.host_str() {
            let credentials = self.auth_storage.get(host);
            let credentials = match credentials {
                Ok(None) => return builder,
                Ok(Some(credentials)) => credentials,
                Err(e) => {
                    tracing::warn!("Error retrieving credentials for {}", host);
                    return builder;    
                },
            };

            match credentials {
                Authentication::BearerToken(token) => {
                    builder.bearer_auth(token)
                },
                Authentication::Basic { username, password } => {
                    builder.basic_auth(username, Some(password))
                },
                Authentication::CondaToken(_token) => {
                    builder
                    // let path = url.path();
                    // let mut new_path = String::new();
                    // new_path.push_str(format!("/t/{}", token).as_str());
                    // new_path.push_str(path);
                    // let mut url = url.clone();
                    // url.set_path(&new_path);
                    // builder.
                },
            }
        } else {
            builder
        }
    }
}
