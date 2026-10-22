#![deny(missing_docs)]

//! Networking utilities for Rattler, specifically authenticating requests
pub use authentication_middleware::AuthenticationMiddleware;
pub use authentication_storage::{
    AuthenticationStorageError, authentication::Authentication, storage::AuthenticationStorage,
};
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
pub use gcs_middleware::{GCSMiddleware, GcsMiddlewareError};

#[cfg(feature = "s3")]
pub mod s3_middleware;
#[cfg(feature = "s3")]
pub use s3_middleware::{S3Middleware, S3MiddlewareError};

#[cfg(all(feature = "rattler_config", not(target_arch = "wasm32")))]
pub use client_config::client_builder_from_config;

pub mod authentication_middleware;
pub mod authentication_storage;
pub mod challenge_middleware;
#[cfg(all(feature = "rattler_config", not(target_arch = "wasm32")))]
pub mod client_config;
pub(crate) mod oauth_refresh;

mod lazy_client;
pub mod mirror_middleware;
pub mod oci_middleware;
pub mod offline_middleware;
pub mod oidc_exchange;
#[cfg(feature = "rattler_config")]
pub mod proxy;
pub mod retry_policies;
#[cfg(all(
    feature = "rattler_config",
    any(feature = "native-tls", feature = "rustls"),
    not(target_arch = "wasm32")
))]
pub mod tls;
pub mod trusted_publishing;
