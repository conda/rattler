#![deny(missing_docs)]

//! Networking utilities for Rattler, specifically authenticating requests
pub use authenticated_client::AuthenticatedClient;

#[cfg(feature = "blocking")]
pub use authenticated_client::AuthenticatedClientBlocking;

pub use authentication_storage::{authentication::Authentication, storage::AuthenticationStorage};

pub mod authenticated_client;
pub mod authentication_storage;
pub mod retry_policies;

mod redaction;

pub use redaction::{
    redact_known_secrets_from_error, redact_known_secrets_from_url, DEFAULT_REDACTION_STR,
};
