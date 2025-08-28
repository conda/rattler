#[cfg(feature = "clap")]
pub mod clap;

use aws_config::{BehaviorVersion, Region};
use aws_credential_types::provider::error::CredentialsError;
use aws_sdk_s3::config::{Credentials, ProvideCredentials};
use rattler_networking::{Authentication, AuthenticationStorage};
use url::Url;

/// Rattler based crates always either use S3 credentials specified by the user
/// through CLI arguments combined with credentials coming from `rattler auth`,
/// or everything is loaded through the AWS SDK.
#[derive(Debug, Clone)]
pub struct S3Credentials {
    /// The endpoint URL of the S3 backend
    pub endpoint_url: Url,

    /// The region of the S3 backend
    pub region: String,

    /// The access key ID for the S3 bucket.
    pub access_key_id: Option<String>,

    /// The secret access key for the S3 bucket.
    pub secret_access_key: Option<String>,

    /// The session token for the S3 bucket.
    pub session_token: Option<String>,
}

/// The resolved S3 credentials.
#[derive(Debug, Clone)]
pub struct ResolvedS3Credentials {
    /// The endpoint URL of the S3 backend
    pub endpoint_url: Url,

    /// The region of the S3 backend
    pub region: String,

    /// The access key ID for the S3 bucket.
    pub access_key_id: String,

    /// The secret access key for the S3 bucket.
    pub secret_access_key: String,

    /// The session token for the S3 bucket.
    pub session_token: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum FromSDKError {
    #[error("No credentials provider found in AWS SDK configuration")]
    NoCredentialsProvider,

    #[error("Could not determine region from AWS SDK configuration")]
    MissingRegion,

    #[error("Could not determine endpoint from AWS SDK configuration")]
    MissingEndpoint,

    #[error("Failed to parse endpoint from AWS SDK configuration")]
    InvalidEndpoint(#[source] url::ParseError),

    #[error(transparent)]
    CredentialsError(CredentialsError),
}

impl ResolvedS3Credentials {
    pub async fn from_sdk() -> Result<Self, FromSDKError> {
        let config = aws_config::load_defaults(BehaviorVersion::latest()).await;

        let region = config
            .region()
            .map(Region::to_string)
            .ok_or(FromSDKError::MissingRegion)?;
        let endpoint_url = match config.endpoint_url() {
            Some(endpoint) => endpoint.parse().map_err(FromSDKError::InvalidEndpoint)?,
            None => return Err(FromSDKError::MissingEndpoint),
        };

        let Some(credentials_provider) = config.credentials_provider() else {
            return Err(FromSDKError::NoCredentialsProvider);
        };
        let credentials: Credentials = credentials_provider
            .provide_credentials()
            .await
            .map_err(FromSDKError::CredentialsError)?;
        let access_key_id = credentials.access_key_id().to_string();
        let secret_access_key = credentials.secret_access_key().to_string();
        let session_token = credentials.session_token().map(ToString::to_string);

        Ok(Self {
            endpoint_url,
            region,
            access_key_id,
            secret_access_key,
            session_token,
        })
    }
}

impl S3Credentials {
    /// Try to resolve the S3 credentials using the provided authentication
    /// storage.
    pub fn resolve(
        self,
        bucket_url: &Url,
        auth_storage: &AuthenticationStorage,
    ) -> Option<ResolvedS3Credentials> {
        let (access_key_id, secret_access_key, session_token) =
            if let (Some(access_key_id), Some(secret_access_key)) =
                (self.access_key_id, self.secret_access_key)
            {
                (access_key_id, secret_access_key, self.session_token)
            } else if let Some((access_key_id, secret_access_key, session_token)) =
                load_s3_credentials_from_auth_storage(auth_storage, bucket_url.clone())
            {
                // Use the credentials from the authentication storage if they are available.
                (access_key_id, secret_access_key, session_token)
            } else {
                return None;
            };

        Some(ResolvedS3Credentials {
            endpoint_url: self.endpoint_url,
            region: self.region,
            access_key_id,
            secret_access_key,
            session_token,
        })
    }
}

fn load_s3_credentials_from_auth_storage(
    auth_storage: &AuthenticationStorage,
    channel: Url,
) -> Option<(String, String, Option<String>)> {
    let auth = auth_storage.get_by_url(channel).ok()?;
    if let (
        _,
        Some(Authentication::S3Credentials {
            access_key_id,
            secret_access_key,
            session_token,
        }),
    ) = auth
    {
        Some((access_key_id, secret_access_key, session_token))
    } else {
        None
    }
}
