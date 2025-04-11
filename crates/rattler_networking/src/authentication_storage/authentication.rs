//! Authentication methods for the conda ecosystem
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The different Authentication methods that are supported in the conda
/// ecosystem
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum Authentication {
    /// A bearer token is sent as a header of the form
    /// `Authorization: Bearer {TOKEN}`
    BearerToken(String),
    /// A basic authentication token is sent as HTTP basic auth
    BasicHTTP {
        /// The username to use for basic auth
        username: String,
        /// The password to use for basic auth
        password: String,
    },
    /// A conda token is sent in the URL as `/t/{TOKEN}/...`
    CondaToken(String),
    /// S3 credentials
    S3Credentials {
        /// The access key ID to use for S3 authentication
        access_key_id: String,
        /// The secret access key to use for S3 authentication
        secret_access_key: String,
        /// The session token to use for S3 authentication
        session_token: Option<String>,
    },
}

/// An error that can occur when parsing an authentication string
#[derive(Debug)]
pub enum AuthenticationParseError {
    /// The scheme is not valid
    InvalidScheme,
    /// The token could not be parsed
    InvalidToken,
}

impl FromStr for Authentication {
    type Err = AuthenticationParseError;

    /// Parse an authentication string into an Authentication struct
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        serde_json::from_str(s).map_err(|_err| AuthenticationParseError::InvalidToken)
    }
}

impl Authentication {
    /// Get the scheme of the authentication method
    pub fn method(&self) -> &str {
        match self {
            Authentication::BearerToken(_) => "BearerToken",
            Authentication::BasicHTTP { .. } => "BasicHTTP",
            Authentication::CondaToken(_) => "CondaToken",
            Authentication::S3Credentials { .. } => "S3",
        }
    }
}
