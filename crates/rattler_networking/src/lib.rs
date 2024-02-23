#![deny(missing_docs)]

//! Networking utilities for Rattler, specifically authenticating requests
pub use authentication_middleware::AuthenticationMiddleware;

pub use authentication_storage::{authentication::Authentication, storage::AuthenticationStorage};

pub mod authentication_middleware;
pub mod authentication_storage;
pub mod retry_policies;

mod redaction;

pub use redaction::{redact_known_secrets_from_url, Redact, DEFAULT_REDACTION_STR};
