//! Authentication methods for the conda ecosystem

#[cfg(test)]
mod tests;
use std::str::FromStr;

use base64::{
    Engine as _,
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    /// OAuth/OIDC credentials with automatic token refresh support
    OAuth {
        /// The OAuth access token
        access_token: String,
        /// The OAuth refresh token (if available)
        refresh_token: Option<String>,
        /// Seconds since UNIX epoch when `access_token` expires
        expires_at: Option<i64>,
        /// Token endpoint URL (cached from OIDC discovery for refresh without
        /// openidconnect)
        token_endpoint: String,
        /// Revocation endpoint URL (RFC 7009, for logout)
        revocation_endpoint: Option<String>,
        /// OAuth client ID
        client_id: String,
        /// Issuer used for authorization. Absent in legacy stored credentials.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        issuer_url: Option<String>,
        /// Granted scopes from the token response (not merely requested scopes).
        /// `None` means unknown; `Some(vec![])` is a known empty grant.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scopes: Option<Vec<String>>,
    },
}

fn oauth_jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| URL_SAFE.decode(payload))
        .ok()?;
    serde_json::from_slice(&decoded).ok()
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
    /// Known OAuth issuer, falling back to unverified legacy JWT metadata.
    /// This is a client-side hint, not proof of token authenticity.
    pub fn oauth_issuer_url(&self) -> Option<String> {
        let Self::OAuth {
            issuer_url,
            access_token,
            ..
        } = self
        else {
            return None;
        };
        issuer_url.clone().or_else(|| {
            oauth_jwt_claims(access_token)?
                .get("iss")?
                .as_str()
                .map(str::to_owned)
        })
    }

    /// Known granted scopes, with an unverified JWT fallback for old records.
    /// Unknown or malformed legacy grants remain unknown, never an empty grant.
    pub fn oauth_scopes(&self) -> Option<Vec<String>> {
        let Self::OAuth {
            scopes,
            access_token,
            ..
        } = self
        else {
            return None;
        };
        if let Some(scopes) = scopes {
            return Some(scopes.clone());
        }
        let claims = oauth_jwt_claims(access_token)?;
        let mut result = None;
        for name in ["scope", "scp"] {
            if let Some(value) = claims.get(name) {
                let scopes = match value {
                    Value::String(value) => {
                        value.split_ascii_whitespace().map(str::to_owned).collect()
                    }
                    Value::Array(values) => values
                        .iter()
                        .map(|v| v.as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()?,
                    _ => return None,
                };
                result.get_or_insert_with(Vec::new).extend(scopes);
            }
        }
        result.map(|mut scopes| {
            scopes.sort();
            scopes.dedup();
            scopes
        })
    }

    /// Get the scheme of the authentication method
    pub fn method(&self) -> &str {
        match self {
            Authentication::BearerToken(_) => "BearerToken",
            Authentication::BasicHTTP { .. } => "BasicHTTP",
            Authentication::CondaToken(_) => "CondaToken",
            Authentication::S3Credentials { .. } => "S3",
            Authentication::OAuth { .. } => "OAuth",
        }
    }
}
