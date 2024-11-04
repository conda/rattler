#![deny(missing_docs)]

//! Networking utilities for Rattler, specifically authenticating requests
pub use authentication_middleware::AuthenticationMiddleware;
pub use authentication_storage::{authentication::Authentication, storage::AuthenticationStorage};
pub use mirror_middleware::MirrorMiddleware;
pub use oci_middleware::OciMiddleware;

#[cfg(feature = "gcs")]
pub mod gcs_middleware;
#[cfg(feature = "gcs")]
pub use gcs_middleware::GCSMiddleware;

pub mod authentication_middleware;
pub mod authentication_storage;

pub mod mirror_middleware;
pub mod oci_middleware;
pub mod retry_policies;
