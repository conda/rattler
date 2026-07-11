#![deny(missing_docs)]

//! Networking utilities for Rattler, specifically authenticating requests
pub use authentication_middleware::AuthenticationMiddleware;
pub use authentication_storage::{authentication::Authentication, storage::AuthenticationStorage};
pub use challenge_middleware::{
    AuthChallengeMiddleware, AuthFlow, AuthFlowError, BearerToken, Challenge,
};
pub use lazy_client::LazyClient;
pub use mirror_middleware::MirrorMiddleware;
pub use oci_middleware::OciMiddleware;
pub use offline_middleware::{OfflineError, OfflineMiddleware};

#[cfg(feature = "gcs")]
pub mod gcs_middleware;
#[cfg(feature = "gcs")]
pub use gcs_middleware::GCSMiddleware;

#[cfg(feature = "s3")]
pub mod s3_middleware;
#[cfg(feature = "s3")]
pub use s3_middleware::S3Middleware;

pub mod authentication_middleware;
pub mod authentication_storage;
pub mod challenge_middleware;
pub(crate) mod oauth_refresh;

mod lazy_client;
pub mod mirror_middleware;
pub mod oci_middleware;
pub mod offline_middleware;
#[cfg(feature = "rattler_config")]
pub mod proxy;
pub mod retry_policies;
pub mod trusted_publishing;
