use std::str::FromStr;

use keyring::{Entry, Result};
use reqwest::{Request, Client, IntoUrl};

#[derive(Debug)]
pub enum Authentication {
    BearerToken(String),
    Basic{username: String, password: String},
    CondaToken(String),
}

#[derive(Debug)]
pub enum AuthenticationParseError {
    InvalidScheme,
    InvalidToken,
}

impl FromStr for Authentication {
    type Err = AuthenticationParseError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let mut parts = s.split_whitespace();
        let scheme = parts.next().unwrap();
        let token = parts.next().unwrap();
        match scheme {
            "Bearer" => Ok(Authentication::BearerToken(token.to_string())),
            "Basic" => {
                let mut token_parts = token.split(":");
                let username = token_parts.next().unwrap();
                let password = token_parts.next().unwrap();
                Ok(Authentication::Basic{username: username.to_string(), password: password.to_string()})
            },
            "CondaToken" => Ok(Authentication::CondaToken(token.to_string())),
            _ => Err(AuthenticationParseError::InvalidScheme),
        }
    }
}

pub fn store_authentication_entry(host: &str, authentication: &Authentication) -> Result<()> {
    let entry = Entry::new("rattler_auth", host)?;
    match authentication {
        Authentication::BearerToken(token) => {
            let password = format!("Bearer {}", token);
            entry.set_password(&password)
        },
        Authentication::Basic{username, password} => {
            let password = format!("Basic {}:{}", username, password);
            entry.set_password(&password)
        },
        Authentication::CondaToken(token) => {
            let password = format!("CondaToken {}", token);
            entry.set_password(&password)
        },
    }
}

pub fn get_authentication(host: &str) -> Result<Option<Authentication>> {
    let entry = Entry::new("rattler_auth", host)?;
    let password = entry.get_password();

    match password {
        Ok(password) => Ok(Some(Authentication::from_str(&password).unwrap())),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn delete_authentication_entry(host: &str) -> Result<()> {
    let entry = Entry::new("rattler_auth", host).unwrap();
    entry.delete_password()
}

pub fn authenticated_request<U: IntoUrl>(client: &Client, url: U) -> reqwest::RequestBuilder {
    let url = url.into_url().unwrap();
    let host = url.host_str().unwrap();

    let authentication = get_authentication(host).unwrap();

    println!("Getting authenticated request for host: {}", host);

    if authentication.is_none() {
        println!("No authentication found for host: {}", host);
        return client.get(url.clone());
    }

    let authentication = authentication.unwrap();

    match authentication {
        Authentication::BearerToken(token) => {
            println!("Using bearer token for host: {}", host);
            client.get(url).bearer_auth(token)
        },
        Authentication::Basic { username, password } => {
            client.get(url).basic_auth(username, Some(password))
        },
        Authentication::CondaToken(token) => {
            let path = url.path();
            let mut new_path = String::new();
            new_path.push_str(format!("/t/{}", token).as_str());
            new_path.push_str(path);
            let mut url = url.clone();
            url.set_path(&new_path);
            client.get(url)
        },
    }
}

pub fn authenticated_request_blocking<U: IntoUrl>(client: &reqwest::blocking::Client, url: U) -> reqwest::blocking::RequestBuilder {
    let url = url.into_url().unwrap();
    let host = url.host_str().unwrap();

    let authentication = get_authentication(host).unwrap();

    if authentication.is_none() {
        return client.get(url.clone());
    }

    let authentication = authentication.unwrap();

    match authentication {
        Authentication::BearerToken(token) => {
            client.get(url).bearer_auth(token)
        },
        Authentication::Basic { username, password } => {
            client.get(url).basic_auth(username, Some(password))
        },
        Authentication::CondaToken(token) => {
            let path = url.path();
            let mut new_path = String::new();
            new_path.push_str(format!("/t/{}", token).as_str());
            new_path.push_str(path);
            let mut url = url.clone();
            url.set_path(&new_path);
            client.get(url)
        },
    }
}